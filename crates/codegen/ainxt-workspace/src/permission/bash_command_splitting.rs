//! Re-export shim over [`ainxt_shell_ast::bash`].
//!
//! The parsing itself moved to `ainxt-shell-ast` so the policy enforcement
//! point can derive capabilities from a command without depending on this
//! crate. Call sites here and in `ainxt-pager` are unchanged, and the original
//! visibility of each item is preserved deliberately: the `pub(crate)` set
//! below was never part of this crate's public API and must not become so.

pub use ainxt_shell_ast::bash::{
    BashCommandHighlights, PlainCommand, all_commands_from_script, heredoc_payload_byte_ranges,
    primary_command_from_script, range_fully_inside, soft_break_chunks,
    soft_break_offsets_after_operators, split_physical_line_at_soft_breaks, try_parse_shell,
    try_parse_word_only_commands_sequence,
};

pub(crate) use ainxt_shell_ast::bash::{
    is_setup_command, is_wrapper_command, strip_wrapper_command, unwrap_wrappers,
    wrapper_has_chdir,
};
