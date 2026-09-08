//! Program → capability classification.
//!
//! These tables do **not** need to be exhaustive to be safe. An unrecognised
//! program still gets [`Capability::ExecuteProcess`], and deny-by-default means
//! it is refused unless the policy allowlists it by name. The tables exist to
//! add *precision* — so a denial can say "because this downloads and then
//! interprets" rather than "because it is not on a list" — and to catch
//! dangerous compositions among programs we do know.
//!
//! Being wrong in the permissive direction here costs nothing: the allowlist is
//! the control, not this file.

use crate::capability::{Capability, Derivation};

/// Programs that open a network connection regardless of arguments.
const NETWORK: &[&str] = &[
    "curl", "wget", "nc", "ncat", "netcat", "telnet", "ftp", "sftp", "scp", "rsync", "ssh",
    "aria2c", "httpie", "http", "https", "socat", "openssl", "nmap", "masscan", "ping", "dig",
    "nslookup", "host", "traceroute", "iperf", "invoke-webrequest", "invoke-restmethod",
    "start-bitstransfer",
];

/// Network programs whose normal purpose is to retrieve bytes.
const DOWNLOAD: &[&str] = &[
    "curl",
    "wget",
    "aria2c",
    "httpie",
    "http",
    "invoke-webrequest",
    "invoke-restmethod",
    "start-bitstransfer",
];

/// Programs that interpret a string as code.
const SHELL_INTERPRETER: &[&str] = &[
    "bash",
    "sh",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "csh",
    "tcsh",
    "eval",
    "source",
    "pwsh",
    "powershell",
    "cmd",
    "invoke-expression",
    "iex",
    "wscript",
    "cscript",
    "mshta",
    "rundll32",
    "regsvr32",
];

const PRIVILEGE: &[&str] = &["sudo", "su", "doas", "pkexec", "runas", "gsudo"];

/// Programs whose purpose is to destroy data.
const DELETE: &[&str] = &[
    "rm",
    "rmdir",
    "unlink",
    "shred",
    "srm",
    "del",
    "erase",
    "remove-item",
    "ri",
    "clear-content",
];

const PROCESS_CONTROL: &[&str] = &[
    "kill",
    "pkill",
    "killall",
    "systemctl",
    "service",
    "launchctl",
    "sc",
    "taskkill",
    "stop-process",
    "start-service",
    "stop-service",
    "shutdown",
    "reboot",
];

/// Programs whose purpose is to read or manage secrets.
const CREDENTIAL: &[&str] = &[
    "gpg",
    "pass",
    "vault",
    "security",
    "keyring",
    "cmdkey",
    "get-credential",
    "aws-vault",
    "op",
    "ssh-add",
    "ssh-keygen",
    "keytool",
];

/// Package managers, mapped to the subcommands that actually fetch and install.
/// An empty subcommand list means every invocation counts.
const PACKAGE_MANAGERS: &[(&str, &[&str])] = &[
    ("pip", &["install", "download", "wheel"]),
    ("pip3", &["install", "download", "wheel"]),
    ("uv", &["add", "pip", "sync", "install"]),
    ("poetry", &["add", "install", "update"]),
    ("conda", &["install", "create", "update"]),
    ("npm", &["install", "i", "add", "ci", "exec", "update"]),
    ("yarn", &["add", "install", "up"]),
    ("pnpm", &["add", "install", "i", "dlx", "update"]),
    ("npx", &[]),
    ("bun", &["add", "install", "i"]),
    ("cargo", &["install", "add", "fetch"]),
    ("go", &["get", "install", "download"]),
    ("gem", &["install", "update"]),
    ("apt", &["install", "update", "upgrade"]),
    ("apt-get", &["install", "update", "upgrade"]),
    ("yum", &["install", "update"]),
    ("dnf", &["install", "update"]),
    ("zypper", &["install", "update"]),
    ("pacman", &["-s", "-sy", "-syu"]),
    ("apk", &["add", "update"]),
    ("brew", &["install", "update", "upgrade", "tap"]),
    ("choco", &["install", "upgrade"]),
    ("winget", &["install", "upgrade"]),
    ("scoop", &["install", "update"]),
    ("dotnet", &["restore", "add"]),
    ("nuget", &["install", "restore"]),
    ("mvn", &[]),
    ("gradle", &[]),
    ("composer", &["install", "require", "update"]),
];

/// `git` subcommands that reach the network, and those that rewrite history or
/// the working tree.
const GIT_NETWORK: &[&str] = &[
    "clone",
    "fetch",
    "pull",
    "push",
    "remote",
    "submodule",
    "ls-remote",
    "archive",
    "request-pull",
];
const GIT_DOWNLOAD: &[&str] = &["clone", "fetch", "pull", "submodule"];
/// Ordinary repository writes. Routine work — must not require approval, or
/// the control prompts on every commit and gets disabled.
const GIT_WRITE: &[&str] = &[
    "commit",
    "push",
    "checkout",
    "switch",
    "restore",
    "merge",
    "cherry-pick",
    "revert",
    "apply",
    "am",
    "clean",
    "add",
    "stash",
    "tag",
];

/// Subcommands that destroy or rewrite history that already exists.
const GIT_HISTORY: &[&str] = &["rebase", "filter-branch", "filter-repo", "reset", "gc", "prune"];

/// Subcommands that push artifacts out to a package registry. Distinct from
/// installing: this is the supply-chain direction.
const PUBLISH_SUBCOMMANDS: &[(&str, &[&str])] = &[
    ("npm", &["publish"]),
    ("yarn", &["publish"]),
    ("pnpm", &["publish"]),
    ("bun", &["publish"]),
    ("cargo", &["publish"]),
    ("gem", &["push"]),
    ("twine", &["upload"]),
    ("mvn", &["deploy"]),
    ("gradle", &["publish"]),
    ("dotnet", &["nuget"]),
    ("nuget", &["push"]),
    ("docker", &["push"]),
    ("poetry", &["publish"]),
    ("uv", &["publish"]),
    ("composer", &["publish"]),
];

/// Path fragments that indicate credential material. Matched case-insensitively
/// against the whole path, so `~/.aws/credentials` and
/// `C:\Users\x\.aws\credentials` both hit.
const CREDENTIAL_PATHS: &[&str] = &[
    ".ssh/",
    ".ssh\\",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    ".aws/",
    ".aws\\",
    ".config/gcloud",
    ".azure/",
    ".azure\\",
    ".kube/config",
    ".kube\\config",
    ".docker/config.json",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".gnupg/",
    ".gnupg\\",
    "keychain",
    "credentials.json",
    ".ainxt/credentials",
    ".ainxt\\credentials",
    ".claude.json",
    "secring",
    "ntuser.dat",
    "/etc/shadow",
    "/etc/sudoers",
];

/// Path prefixes owned by the OS or another tenant.
const SYSTEM_PATHS: &[&str] = &[
    "/etc/",
    "/usr/",
    "/bin/",
    "/sbin/",
    "/boot/",
    "/sys/",
    "/proc/",
    "/var/lib/",
    "/library/",
    "/system/",
    "c:\\windows",
    "c:/windows",
    "c:\\program files",
    "c:/program files",
    "c:\\programdata",
    "hklm",
    "hkey_local_machine",
];

/// Normalise a program token to a bare lower-case name: strips any directory
/// prefix and a Windows executable suffix, so `/usr/bin/curl`, `curl.exe` and
/// `CURL` all classify identically.
pub fn normalize_program(word: &str) -> String {
    let base = ainxt_shell_ast::access::shell_program_name(word);
    let base = base.to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".com"] {
        if let Some(stripped) = base.strip_suffix(suffix) {
            return stripped.to_owned();
        }
    }
    base
}

fn has_subcommand(args: &[String], candidates: &[&str]) -> bool {
    // The subcommand is the first token that is not a flag. Scanning rather than
    // taking `args[0]` keeps `git -C /repo clone ...` classified correctly.
    args.iter()
        .find(|a| !a.starts_with('-'))
        .map(|a| candidates.contains(&a.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Programs that will interpret whatever arrives on stdin or in a named file
/// as code.
///
/// Broader than [`SHELL_INTERPRETER`] on purpose: `curl … | python3` is exactly
/// as dangerous as `curl … | bash`, but `python3 script.py` is ordinary work
/// and must not be treated as shell interpretation everywhere else.
const CODE_INTERPRETERS: &[&str] = &[
    "bash",
    "sh",
    "zsh",
    "dash",
    "ksh",
    "fish",
    "csh",
    "tcsh",
    "python",
    "python2",
    "python3",
    "perl",
    "ruby",
    "node",
    "php",
    "rscript",
    "pwsh",
    "powershell",
    "cmd",
];

pub fn is_code_interpreter(program: &str) -> bool {
    CODE_INTERPRETERS.contains(&program)
}

/// Whether this invocation retrieves remote content.
pub fn is_downloader(program: &str, args: &[String]) -> bool {
    if DOWNLOAD.contains(&program) {
        return true;
    }
    // `git clone`/`fetch`/`pull` land a tree on disk, which is the same
    // fetch-then-run vector by another route.
    program == "git" && has_subcommand(args, GIT_DOWNLOAD)
}

/// The script string handed to an interpreter via `-c`, if any.
///
/// Used both to recurse capability derivation into nested scripts and to stop
/// `bash -c "…"` being mistaken for an interpreter reading downloaded input.
pub fn dash_c_script(args: &[String]) -> Option<&str> {
    args.iter()
        .position(|a| a == "-c" || a == "--command")
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

/// Whether any of `flags` appears among the arguments.
fn has_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|a| {
        let lower = a.to_ascii_lowercase();
        flags.contains(&lower.as_str())
    })
}

pub fn path_is_credential(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    CREDENTIAL_PATHS.iter().any(|frag| lower.contains(frag))
}

pub fn path_is_system(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    SYSTEM_PATHS.iter().any(|prefix| lower.contains(prefix))
}

/// Launchers that run *another* program. Their own name tells you almost
/// nothing, so classification must continue into what they launch — otherwise
/// `sudo curl ...` classifies as privilege escalation and nothing else, and the
/// network reach is invisible.
///
/// `timeout`/`env`/`nice` are already peeled upstream by
/// `ainxt_shell_ast::bash::unwrap_wrappers`; these are the ones it leaves.
const LAUNCHERS: &[&str] = &[
    "sudo", "doas", "pkexec", "runas", "gsudo", "nohup", "setsid", "exec", "xargs", "command",
    "builtin", "time", "watch",
];

/// Launcher flags that take a separate value, which must be skipped along with
/// the flag so the value is not mistaken for the launched program
/// (`sudo -u root curl` must land on `curl`, not `root`).
const LAUNCHER_VALUE_FLAGS: &[&str] = &[
    "-u", "--user", "-g", "--group", "-p", "--prompt", "-C", "-D", "--chdir", "-n", "-I",
];

/// Attribute capabilities for one already-split command.
///
/// `words[0]` is the program; the remainder are its arguments. Wrapper peeling
/// (`timeout`, `env`, ...) is the caller's job — by the time we get here the
/// program should be the real one.
pub fn classify(words: &[String], out: &mut Derivation) {
    classify_depth(words, out, 0);
}

/// Depth cap exists only to bound pathological nesting (`sudo sudo sudo ...`);
/// legitimate commands never approach it.
fn classify_depth(words: &[String], out: &mut Derivation, depth: usize) {
    if depth > 4 {
        out.add(
            Capability::ExecuteProcess,
            "launcher nesting exceeded the inspection depth",
        );
        return;
    }
    let Some(raw) = words.first() else {
        return;
    };
    let program = normalize_program(raw);
    let args = &words[1..];

    out.targets.add_program(program.clone());
    out.add(
        Capability::ExecuteProcess,
        format!("runs `{program}`"),
    );

    if NETWORK.contains(&program.as_str()) {
        out.add(
            Capability::NetworkConnect,
            format!("`{program}` opens a network connection"),
        );
    }
    if DOWNLOAD.contains(&program.as_str()) {
        out.add(
            Capability::Download,
            format!("`{program}` retrieves remote content"),
        );
    }
    if SHELL_INTERPRETER.contains(&program.as_str()) {
        out.add(
            Capability::ShellInterpretation,
            format!("`{program}` interprets its input as code"),
        );
    }
    if PRIVILEGE.contains(&program.as_str()) {
        out.add(
            Capability::PrivilegeEscalation,
            format!("`{program}` elevates privileges"),
        );
    }
    if PROCESS_CONTROL.contains(&program.as_str()) {
        out.add(
            Capability::ProcessControl,
            format!("`{program}` controls processes or services"),
        );
    }
    if CREDENTIAL.contains(&program.as_str()) {
        out.add(
            Capability::CredentialAccess,
            format!("`{program}` reads or manages secrets"),
        );
    }
    if DELETE.contains(&program.as_str()) {
        out.add(Capability::FsDelete, format!("`{program}` deletes files"));
    }

    if let Some((_, subs)) = PACKAGE_MANAGERS.iter().find(|(p, _)| *p == program)
        && (subs.is_empty() || has_subcommand(args, subs))
    {
        out.add(
            Capability::InstallPackage,
            format!("`{program}` installs packages"),
        );
        out.add(
            Capability::NetworkConnect,
            format!("`{program}` fetches from a package registry"),
        );
        out.add(
            Capability::Download,
            format!("`{program}` downloads package contents"),
        );
    }

    if program == "git" {
        if has_subcommand(args, GIT_NETWORK) {
            out.add(Capability::NetworkConnect, "git contacts a remote");
        }
        if has_subcommand(args, GIT_DOWNLOAD) {
            out.add(Capability::Download, "git retrieves remote objects");
        }
        if has_subcommand(args, GIT_WRITE) {
            out.add(Capability::ModifyGit, "git writes to the repository");
        }
        if has_subcommand(args, GIT_HISTORY) {
            out.add(
                Capability::RewriteGitHistory,
                "git rewrites history that already exists",
            );
        }
        // `commit --amend` replaces a commit rather than adding one, so it
        // belongs with history rewriting despite looking like a plain commit.
        if has_subcommand(args, &["commit"]) && has_flag(args, &["--amend"]) {
            out.add(
                Capability::RewriteGitHistory,
                "`git commit --amend` replaces an existing commit",
            );
        }
        if has_subcommand(args, &["push"]) {
            out.add(Capability::Upload, "git push sends objects to a remote");
            if has_flag(args, &["--force", "-f", "--force-with-lease"]) {
                out.add(
                    Capability::ForcePush,
                    "force push overwrites remote history",
                );
            }
        }
    }

    if let Some((_, subs)) = PUBLISH_SUBCOMMANDS.iter().find(|(p, _)| *p == program)
        && has_subcommand(args, subs)
    {
        out.add(
            Capability::PublishPackage,
            format!("`{program}` publishes an artifact to a registry"),
        );
        out.add(
            Capability::Upload,
            format!("`{program}` uploads to a registry"),
        );
        out.add(
            Capability::NetworkConnect,
            format!("`{program}` contacts a registry"),
        );
    }

    // A URL anywhere in the arguments implies network reach even for a program
    // we do not recognise — this is what catches novel fetchers.
    for arg in args {
        if let Some(url) = as_url(arg) {
            out.targets.add_url(url.clone());
            out.add(
                Capability::NetworkConnect,
                format!("argument names a remote endpoint: {url}"),
            );
        }
    }

    // `sudo -c` / `su -c` hand a *script* to a shell. The launcher recursion
    // below deliberately refuses to treat that string as a program name, so
    // record the interpretation here or it would be lost entirely.
    if (LAUNCHERS.contains(&program.as_str()) || PRIVILEGE.contains(&program.as_str()))
        && args.iter().any(|a| a == "-c" || a == "--command")
    {
        out.add(
            Capability::ShellInterpretation,
            format!("`{program} -c` interprets a script string"),
        );
    }

    // Continue into whatever a launcher launches.
    if LAUNCHERS.contains(&program.as_str())
        && let Some(rest) = launched_command(args)
    {
        out.add(
            Capability::ExecuteProcess,
            format!("`{program}` launches another program"),
        );
        classify_depth(rest, out, depth + 1);
    }
}

/// The sub-command a launcher runs: skip the launcher's own flags (and their
/// values) and return the remainder.
///
/// A `-c` flag is the exception — its value is a *script*, not a program, so we
/// record the interpretation and stop rather than misclassifying the script
/// text as an executable name.
fn launched_command(args: &[String]) -> Option<&[String]> {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        if !arg.starts_with('-') {
            break;
        }
        if arg == "-c" || arg == "--command" {
            return None;
        }
        if LAUNCHER_VALUE_FLAGS.contains(&arg.as_str()) {
            i += 2;
        } else {
            i += 1;
        }
    }
    args.get(i..).filter(|rest| !rest.is_empty())
}

/// Recognise an argument that names a remote endpoint, including the shorthand
/// forms package managers accept (`git+https://`, `user@host:path`).
fn as_url(arg: &str) -> Option<String> {
    const SCHEMES: &[&str] = &[
        "http://",
        "https://",
        "ftp://",
        "ftps://",
        "ssh://",
        "git://",
        "git+https://",
        "git+ssh://",
        "svn://",
        "rsync://",
    ];
    let lower = arg.to_ascii_lowercase();
    if SCHEMES.iter().any(|s| lower.starts_with(s)) {
        return Some(arg.to_owned());
    }
    // `user@host:path` (scp/git shorthand). Excludes Windows drive letters and
    // bare `a:b` by requiring an `@` before the colon and a dot in the host.
    if let Some((userhost, _path)) = arg.split_once(':')
        && let Some((_user, host)) = userhost.split_once('@')
        && host.contains('.')
        && !host.contains('/')
    {
        return Some(arg.to_owned());
    }
    None
}
