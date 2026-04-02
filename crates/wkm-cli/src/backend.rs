/// Dispatch to the appropriate git backend based on detected VCS.
///
/// Usage:
/// ```ignore
/// with_backend!(ctx, cwd, git => {
///     sync::sync(&ctx, &git)?;
/// })
/// ```
///
/// When the repo is a colocated jj+git repo with `jj` on PATH,
/// `$git` is bound to a `JjCli` instance. Otherwise, it's a `CliGit`.
/// Both implement the same git traits, so the body compiles with either type.
macro_rules! with_backend {
    ($ctx:expr, $cwd:expr, $git:ident => $body:expr) => {{
        use wkm_core::repo::VcsBackend;
        match $ctx.vcs_backend {
            VcsBackend::JjColocated => {
                let $git = wkm_core::git::jj_cli::JjCli::new($cwd);
                $body
            }
            VcsBackend::Git => {
                let $git = wkm_core::git::cli::CliGit::new($cwd);
                $body
            }
        }
    }};
}

pub(crate) use with_backend;
