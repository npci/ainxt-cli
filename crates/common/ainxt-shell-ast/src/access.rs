//! Pure AST/path derivation for shell commands: which files a command reads or
//! writes, including redirects, `tee`, `dd of=`, in-place `sed`, and
//! package-manager launchers.
//!
//! Extracted verbatim from `ainxt_workspace::permission::shell_access` so the
//! policy enforcement point can derive capabilities without depending on the
//! permission crate. Contains no policy types and makes no decisions — callers
//! map `ShellFileMode` onto their own access vocabulary.

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use crate::bash::unwrap_wrappers;

/// Write paths from a SINGLE already-split command's words (no redirects — the
/// caller handles those at the tree level). Wrapper-aware. Reused both per parsed
/// command and to re-check the inner command of a package-manager launcher
/// (`uv run`, `npm exec`, ...) whose writes the outer program name would hide.
pub fn command_words_write_paths(words: &[String]) -> Vec<String> {
    let inner = unwrap_wrappers(words);
    let mut out = Vec::new();
    let Some(program) = inner.first().map(|w| shell_program_name(w)) else {
        return out;
    };
    let program = program.to_ascii_lowercase();

    // Flag-named write operands (`dd of=`, `sort`/`go`/`rustc -o`, `git --output`).
    for (path, mode) in special_file_operands(&program, inner) {
        if matches!(mode, ShellFileMode::Write) {
            out.push(path);
        }
    }
    // Path-moving destinations (`cp`/`mv`/`ln`/`install` dest; `rm`/`touch`/…;
    // `uniq` output operand).
    if let Some(operands) = shell_path_command_operands(&program, inner) {
        for (path, mode) in operands {
            if matches!(mode, ShellFileMode::Write) {
                out.push(path.to_owned());
            }
        }
        return out;
    }
    // Named-argument writers (`tee`/`truncate`/...) and in-place `sed -i`, which
    // rewrites each file operand.
    let writes_operands = matches!(shell_file_mode(&program), Some(ShellFileMode::Write))
        || (program == "sed" && shell_sed_in_place(inner));
    if writes_operands {
        for token in shell_file_candidates(inner) {
            out.push(token.to_owned());
        }
    }
    out
}

/// Every path a shell command WRITES, from an ALREADY-PARSED tree (so a caller
/// that already parsed `src` shares the one parse): output redirects plus the
/// per-command writers from [`command_words_write_paths`] (`dd of=`, `sort -o`,
/// `git --output`, `cp`/`mv` dest, `tee`/`truncate`, in-place `sed`/`rustfmt`,
/// `uniq` output, ...). No safe-sink filtering — the caller decides.
pub fn command_write_paths_in_tree(root: Node<'_>, src: &str) -> Vec<String> {
    let mut out = Vec::new();

    // Output redirects (`> f`, `>> f`); fd-dups/heredocs are already skipped.
    for (_start, path, mode, _ambiguous) in shell_redirect_targets(root, src) {
        if matches!(mode, ShellFileMode::Write)
            && let Some(path) = path
        {
            out.push(path);
        }
    }
    // Per-command writers, after peeling env/timeout/... wrappers.
    for (_start, raw_words, _ambiguous) in shell_command_invocations(root, src) {
        out.extend(command_words_write_paths(&raw_words));
    }
    out
}

#[derive(Clone, Copy)]
pub enum ShellFileMode {
    Read,
    Write,
}

/// Tools that read/write a file named as an argument. Not exhaustive — redirects
/// are the robust catch-all (caught via the AST for any program).
pub fn shell_file_mode(program: &str) -> Option<ShellFileMode> {
    match program {
        "cat" | "tac" | "nl" | "head" | "tail" | "grep" | "egrep" | "fgrep" | "rg" | "sed"
        | "awk" | "less" | "more" | "bat" | "strings" | "xxd" | "od" | "hexdump" | "base64"
        | "base32" | "cut" | "sort" | "uniq" | "wc" | "type" | "get-content" | "gc" | "diff"
        | "comm" | "rev" | "jq" | "yq" | "select-string" | "sls" | "ag" | "ack" | "zcat"
        | "zless" | "zmore" | "zgrep" | "zegrep" | "zfgrep" | "bzcat" | "bzgrep" | "xzcat"
        | "xzgrep" | "zstdcat" | "lz4cat" => Some(ShellFileMode::Read),
        "tee" | "set-content" | "out-file" | "add-content" | "tee-object" | "truncate" => {
            Some(ShellFileMode::Write)
        }
        _ => None,
    }
}

pub fn shell_program_name(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

/// True if a command runs in the current shell (reaching later commands), not in
/// a subshell/pipeline/substitution or backgrounded.
pub fn runs_in_current_shell(cmd: Node<'_>) -> bool {
    let mut node = cmd;
    loop {
        if node.next_sibling().is_some_and(|s| s.kind() == "&") {
            return false; // backgrounded subshell
        }
        let Some(parent) = node.parent() else {
            return true;
        };
        if matches!(
            parent.kind(),
            "subshell" | "pipeline" | "command_substitution" | "process_substitution"
        ) {
            return false;
        }
        node = parent;
    }
}

/// Source positions of in-shell `cd`/`pushd`/`popd`. We don't resolve the new
/// directory; a relative operand after one is unpinnable → Ask.
pub fn cwd_poison_positions(root: Node<'_>, src: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "command"
            && node
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(src.as_bytes()).ok())
                .is_some_and(|p| matches!(shell_program_name(p), "cd" | "pushd" | "popd"))
            && runs_in_current_shell(node)
        {
            positions.push(node.start_byte());
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    positions
}

/// Whether an operand at `at` runs after a cwd change, so it can't be pinned.
pub fn cwd_unpinned_before(positions: &[usize], at: usize) -> bool {
    positions.iter().any(|&p| p < at)
}

/// A command operand or redirect destination extracted from the AST.
pub enum ArgText {
    /// Literal path/word, no runtime expansion.
    Literal(String),
    /// Runtime expansion; unpinnable, so callers prompt.
    Ambiguous,
}

/// True if any descendant expands at runtime (e.g. `$X` in `.e"$X"`), so the text
/// isn't a literal path.
pub fn node_has_expansion(node: Node<'_>) -> bool {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        for i in 0..n.child_count() {
            let Some(child) = n.child(i) else { continue };
            if matches!(
                child.kind(),
                "expansion"
                    | "simple_expansion"
                    | "command_substitution"
                    | "arithmetic_expansion"
                    | "process_substitution"
            ) {
                return true;
            }
            stack.push(child);
        }
    }
    false
}

/// Literal text of an operand node, or `Ambiguous` if it expands at runtime;
/// `None` for non-operands (e.g. a leading `VAR=value`).
pub fn shell_node_arg(node: Node<'_>, src: &str) -> Option<ArgText> {
    let text = || node.utf8_text(src.as_bytes()).ok().map(str::to_owned);
    match node.kind() {
        "variable_assignment" => None,
        "word" | "number" => text().map(ArgText::Literal),
        "raw_string" => {
            let raw = node.utf8_text(src.as_bytes()).ok()?;
            let stripped = raw
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(raw);
            Some(ArgText::Literal(stripped.to_owned()))
        }
        "string" => {
            if node_has_expansion(node) {
                return Some(ArgText::Ambiguous);
            }
            let raw = node.utf8_text(src.as_bytes()).ok()?;
            let stripped = raw
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(raw);
            Some(ArgText::Literal(stripped.to_owned()))
        }
        "concatenation" => {
            if node_has_expansion(node) {
                Some(ArgText::Ambiguous)
            } else {
                text().map(ArgText::Literal)
            }
        }
        _ => Some(ArgText::Ambiguous),
    }
}

/// Every `command` node (incl. nested) as `(start_byte, words, ambiguous)`, in
/// source order. `start_byte` orders invocations against cwd-change positions.
pub fn shell_command_invocations(root: Node<'_>, src: &str) -> Vec<(usize, Vec<String>, bool)> {
    let mut found: Vec<(usize, Vec<String>, bool)> = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "command" {
            let mut words = Vec::new();
            let mut ambiguous = false;
            for i in 0..node.named_child_count() {
                let Some(child) = node.named_child(i) else {
                    continue;
                };
                let operand = if child.kind() == "command_name" {
                    child
                        .named_child(0)
                        .and_then(|inner| shell_node_arg(inner, src))
                } else {
                    shell_node_arg(child, src)
                };
                match operand {
                    Some(ArgText::Literal(w)) => words.push(w),
                    Some(ArgText::Ambiguous) => ambiguous = true,
                    None => {}
                }
            }
            found.push((node.start_byte(), words, ambiguous));
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    found.sort_by_key(|(start, _, _)| *start);
    found
}

/// Every `file_redirect` target as `(start_byte, path, mode, ambiguous)`; skips
/// heredocs/fd-dups that touch no named file.
pub fn shell_redirect_targets(
    root: Node<'_>,
    src: &str,
) -> Vec<(usize, Option<String>, ShellFileMode, bool)> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if node.kind() == "file_redirect"
            && let Some((path, mode, ambiguous)) = shell_redirect_one(node, src)
        {
            out.push((node.start_byte(), path, mode, ambiguous));
        }
        for i in 0..node.child_count() {
            if let Some(child) = node.child(i) {
                stack.push(child);
            }
        }
    }
    out
}

pub fn shell_redirect_one(node: Node<'_>, src: &str) -> Option<(Option<String>, ShellFileMode, bool)> {
    let mut mode = None;
    for i in 0..node.child_count() {
        let kind = node.child(i)?.kind();
        // `<<`/`<<<` read from inline text, not a file.
        if kind.contains("<<") {
            return None;
        }
        if kind.contains('>') {
            mode = Some(ShellFileMode::Write);
            break;
        }
        if kind.contains('<') {
            mode = Some(ShellFileMode::Read);
            break;
        }
    }
    let mode = mode?;
    let dest = node.child_by_field_name("destination")?;
    match shell_node_arg(dest, src)? {
        ArgText::Literal(s) => {
            // Skip fd duplications (`>&1`) and empty targets.
            if s.is_empty() || s.starts_with('&') || s.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                let ambiguous = shell_arg_is_ambiguous(&s);
                Some((Some(s), mode, ambiguous))
            }
        }
        ArgText::Ambiguous => Some((None, mode, true)),
    }
}

pub fn shell_sed_in_place(words: &[String]) -> bool {
    words.iter().skip(1).any(|word| {
        word == "--in-place"
            || word.starts_with("--in-place=")
            // `i` is sed's only short flag with that letter → any `-…i…` is in-place.
            || (word.starts_with('-') && !word.starts_with("--") && word.contains('i'))
    })
}

pub fn shell_output_flag_values(words: &[String]) -> impl Iterator<Item = &str> {
    words.iter().enumerate().filter_map(|(i, token)| {
        token
            .strip_prefix("--output=")
            .or_else(|| token.strip_prefix("-o").filter(|value| !value.is_empty()))
            .or_else(|| {
                (token == "--output" || token == "-o")
                    .then(|| words.get(i + 1).map(String::as_str))
                    .flatten()
            })
    })
}

/// Values of a value-taking flag written as `flag=v`, `flag v`, or — for short
/// flags only — glued `flagv` (e.g. `-ov`). Long (`--`) flags match `--flag=v` /
/// `--flag v` only (no glued form).
pub fn value_flag_values<'a>(words: &'a [String], flag: &str) -> Vec<&'a str> {
    let eq_prefix = format!("{flag}=");
    words
        .iter()
        .enumerate()
        .filter_map(|(i, token)| {
            if let Some(value) = token.strip_prefix(&eq_prefix) {
                Some(value)
            } else if token == flag {
                words.get(i + 1).map(String::as_str)
            } else if !flag.starts_with("--") {
                token.strip_prefix(flag).filter(|value| !value.is_empty())
            } else {
                None
            }
        })
        .collect()
}

/// Flag-named file operands (not positionals): `dd`'s `if=`/`of=` (read/write),
/// `sort`/`go`'s `-o`/`--output` build output, `git`'s `--output`/`-o`/`-O`, and
/// `rustfmt`'s file operands (rewritten in place). Empty for other programs.
pub fn special_file_operands(program: &str, words: &[String]) -> Vec<(String, ShellFileMode)> {
    match program {
        "dd" => words
            .iter()
            .skip(1)
            .filter_map(|token| {
                token
                    .strip_prefix("if=")
                    .map(|path| (path.to_owned(), ShellFileMode::Read))
                    .or_else(|| {
                        token
                            .strip_prefix("of=")
                            .map(|path| (path.to_owned(), ShellFileMode::Write))
                    })
            })
            .collect(),
        // `--output`/`-o` write the output file. (`git`'s `-O` is a READ
        // order-file, NOT a write, so it is intentionally excluded.)
        "sort" | "go" | "git" => shell_output_flag_values(words)
            .map(|output| (output.to_owned(), ShellFileMode::Write))
            .collect(),
        // `rustc` writes its compiled output via `-o`/`--out-dir` (matches `go`).
        "rustc" => shell_output_flag_values(words)
            .chain(value_flag_values(words, "--out-dir"))
            .map(|output| (output.to_owned(), ShellFileMode::Write))
            .collect(),
        // `rustfmt` rewrites each file operand in place (like an always-on
        // `sed -i`), so its non-flag operands are writes.
        "rustfmt" => shell_file_candidates(words)
            .into_iter()
            .map(|path| (path.to_owned(), ShellFileMode::Write))
            .collect(),
        _ => Vec::new(),
    }
}


/// Operands that may name a file. After a bare `--`, tokens are positional even if
/// `-`-prefixed (`rm -- -/../.env`). `=`-names are kept (a real `VAR=value` is
/// already dropped by the AST).
pub fn shell_file_candidates(words: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut end_of_options = false;
    for token in words.iter().skip(1) {
        if !end_of_options && token == "--" {
            end_of_options = true;
            continue;
        }
        if end_of_options || (token != "-" && !token.starts_with('-')) {
            out.push(token.as_str());
        }
    }
    out
}

/// File operands implied by path-moving commands. `cp`/`mv`/`ln`/`install` read
/// source(s) and write the destination; `rm`/`rmdir`/`mkdir`/`touch` write every
/// operand; `None` otherwise. (`chmod`/`chown` touch metadata, not content.)
pub fn shell_path_command_operands<'a>(
    program: &str,
    words: &'a [String],
) -> Option<Vec<(&'a str, ShellFileMode)>> {
    match program {
        "cp" | "mv" | "ln" | "install" => {
            // Last positional is the destination (Write), the rest sources (Read).
            // The rare `-t DIR` reorder isn't parsed — bounded since denies match
            // by basename.
            let operands = shell_file_candidates(words);
            let (dest, sources) = operands.split_last()?;
            Some(
                sources
                    .iter()
                    .map(|s| (*s, ShellFileMode::Read))
                    .chain(std::iter::once((*dest, ShellFileMode::Write)))
                    .collect(),
            )
        }
        "rm" | "rmdir" | "mkdir" | "touch" => Some(
            shell_file_candidates(words)
                .into_iter()
                .map(|c| (c, ShellFileMode::Write))
                .collect(),
        ),
        // `uniq [INPUT [OUTPUT]]`: a 2nd positional is the output file (Write);
        // the 1st is the input (Read). Fewer operands use stdin/stdout.
        "uniq" => match shell_file_candidates(words).as_slice() {
            [input, output, ..] => Some(vec![
                (*input, ShellFileMode::Read),
                (*output, ShellFileMode::Write),
            ]),
            _ => None,
        },
        _ => None,
    }
}

pub fn shell_arg_is_ambiguous(token: &str) -> bool {
    token.contains('*') || token.contains('?') || token.contains('[')
}

/// A recursive directory search can't pin its operands → prompt. `rg`/`ag`/`ack`
/// recurse given no path or a directory operand (`candidates[0]` is the pattern);
/// grep only with `-r`/`-R`.
pub fn shell_reader_can_recurse(program: &str, words: &[String], candidates: &[&str]) -> bool {
    let grep_recursive = matches!(program, "grep" | "egrep" | "fgrep")
        && words.iter().any(|word| {
            word == "--recursive"
                || word == "--dereference-recursive"
                || (word.starts_with('-')
                    && !word.starts_with("--")
                    && (word.contains('r') || word.contains('R')))
        });
    let searches_dir = matches!(program, "rg" | "ag" | "ack")
        && (candidates.len() <= 1 || candidates.iter().skip(1).any(|c| is_directory_operand(c)));
    grep_recursive || searches_dir
}

/// A path that syntactically names a directory (so a recursive reader descends it).
pub fn is_directory_operand(token: &str) -> bool {
    token == "." || token == ".." || token.ends_with('/')
}

pub fn is_absolute_shell_path(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with("~/")
        || path.as_bytes().get(1).is_some_and(|b| *b == b':')
}

pub fn normalize_shell_path(path: &str) -> String {
    lexical_normalize(&normalize_shell_path_raw(path))
}

/// Quote/backslash/`/c/` normalization WITHOUT collapsing `.`/`..`, so symlink
/// resolution can follow `..` *physically* (after the link) rather than have it
/// erased textually before the link is ever seen.
pub fn normalize_shell_path_raw(path: &str) -> String {
    let p = path.trim_matches(['\"', '\'']).replace('\\', "/");
    match p.strip_prefix("/c/") {
        Some(rest) => format!("C:/{rest}"),
        None => p,
    }
}

pub fn lexical_normalize(path: &str) -> String {
    let prefix_len = if path.as_bytes().get(1).is_some_and(|b| *b == b':') {
        2
    } else {
        0
    };
    let (prefix, rest) = path.split_at(prefix_len);
    let absolute = rest.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for segment in rest.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|s| *s != "..") {
                    out.pop();
                } else if !absolute {
                    out.push("..");
                }
            }
            segment => out.push(segment),
        }
    }
    let body = out.join("/");
    match (prefix.is_empty(), absolute, body.is_empty()) {
        (false, true, false) => format!("{prefix}/{body}"),
        (false, true, true) => format!("{prefix}/"),
        (false, false, _) => format!("{prefix}{body}"),
        (true, true, false) => format!("/{body}"),
        (true, true, true) => "/".to_owned(),
        (true, false, _) => body,
    }
}

/// Whether *any* existing component of `absolute` is a symlink — used to fail
/// closed (Ask) when a linky operand can't be fully resolved, including a
/// mid-path link (not just the leaf).
pub fn path_has_symlink(absolute: &str) -> bool {
    let path = Path::new(absolute);
    if !path.is_absolute() {
        return false;
    }
    let mut prefix = PathBuf::new();
    for comp in path.components() {
        prefix.push(comp);
        if std::fs::symlink_metadata(&prefix)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Resolve a filesystem-absolute operand to its real symlink target. `None` for
/// relative/unanchorable inputs. Input must be absolute so resolution anchors to
/// the command's cwd, not the process cwd. Point-in-time (TOCTOU) only.
pub fn resolve_symlink_target(absolute: &str) -> Option<String> {
    let path = Path::new(absolute);
    if !path.is_absolute() {
        return None;
    }
    let resolved = resolve_following_symlinks(path, 0)?;
    // `/`-normalize so the result matches rule text on Windows (backslash form).
    Some(normalize_shell_path(&resolved.to_string_lossy()))
}

/// Resolve `path` following every symlink, including a *dangling* final link
/// (which `canonicalize` alone rejects) and not-yet-existing trailing
/// components. Depth-bounded against cycles; any fs error yields `None`.
/// Blocking fs syscalls; runs per operand when file rules exist.
pub fn resolve_following_symlinks(path: &Path, depth: usize) -> Option<PathBuf> {
    const MAX_SYMLINK_DEPTH: usize = 40;
    if depth > MAX_SYMLINK_DEPTH {
        return None;
    }
    // `dunce` avoids Windows `\\?\` unchanged paths (repo convention).
    if let Ok(canonical) = dunce::canonicalize(path) {
        return Some(canonical);
    }
    // Resolve the parent, then the final component, so a dangling/new leaf still follows.
    let parent = path.parent()?;
    let file_name = path.file_name()?;
    let resolved_parent = resolve_following_symlinks(parent, depth + 1)?;
    let candidate = resolved_parent.join(file_name);
    if let Ok(meta) = std::fs::symlink_metadata(&candidate)
        && meta.file_type().is_symlink()
    {
        // A symlink must be followed; if it can't be read, treat the whole path
        // as unresolved (`None`) rather than returning the link's own path.
        let target = std::fs::read_link(&candidate).ok()?;
        let target = if target.is_absolute() {
            target
        } else {
            resolved_parent.join(target)
        };
        return resolve_following_symlinks(&target, depth + 1);
    }
    Some(candidate)
}

