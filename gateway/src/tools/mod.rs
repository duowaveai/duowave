//! The developer's commands beside `serve`: the Node gateway's dev tools (src/tools/*.ts — `pnpm replay`, `journal`,
//! `animations`, `call-sim`, `livekit-token`; removed from the tree: git tag `node-gateway-last`) as `duowave <command>`.
//! Each reads the configuration as `serve` does — the environment over the `.env` files — and does its work with the
//! gateway's own modules: the speech pipeline, the session journal, the calls hub's protocol, the LiveKit token.
//!
//! The station's build has no calls: `call-sim` and `livekit-token` are the PC's only.

pub mod animations;
#[cfg(feature = "pc")]
pub mod call_sim;
pub mod journal;
#[cfg(feature = "pc")]
pub mod livekit_token;
pub mod replay;

use crate::config::{Config, Env};

/// The commands, as `duowave <command>` takes them.
pub const COMMANDS: &[&str] = &["replay", "journal", "animations", "call-sim", "livekit-token"];

/// A command line that is not one: printed as it is, exit code 2.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Usage(pub String);

/// A failure the tool explains in its own words (the TS's `console.error` and exit 1): printed as it is.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Failure(pub String);

/// Runs a command with its arguments (those after the command's name); the exit code. What fails is printed on
/// stderr: a tool's own message as it is, anything else after `duowave <command>:`.
pub fn command(name: &str, args: &[String]) -> i32 {
    // One process-wide TLS provider, as `serve` has: recognition in the cloud, a wss:// gateway.
    let _ = rustls::crypto::ring::default_provider().install_default();
    match run(name, args) {
        Ok(()) => 0,
        Err(error) if error.is::<Usage>() => {
            eprintln!("{error}");
            2
        }
        Err(error) if error.is::<Failure>() => {
            eprintln!("{error}");
            1
        }
        Err(error) => {
            eprintln!("duowave {name}: {error:#}");
            1
        }
    }
}

fn run(name: &str, args: &[String]) -> anyhow::Result<()> {
    match name {
        "replay" => replay::main(args),
        "journal" => journal::main(args),
        "animations" => animations::main(args),
        #[cfg(feature = "pc")]
        "call-sim" => call_sim::main(args),
        #[cfg(feature = "pc")]
        "livekit-token" => livekit_token::main(args),
        #[cfg(not(feature = "pc"))]
        "call-sim" | "livekit-token" => {
            Err(Usage(format!("duowave {name}: not in this build — the station's build has no calls")).into())
        }
        other => Err(Usage(format!("duowave: unknown command {other:?}")).into()),
    }
}

/// The value after `--<name>`, as the TS tools' `argument()` took it.
pub fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let flag = format!("--{name}");
    let index = args.iter().position(|arg| *arg == flag)?;
    args.get(index + 1).map(String::as_str)
}

/// `--<name>` is given.
pub fn flag(args: &[String], name: &str) -> bool {
    args.contains(&format!("--{name}"))
}

/// The configuration as `serve` reads it: the process environment over DATA_DIR/.env and ./.env.
pub fn config() -> anyhow::Result<Config> {
    Ok(Config::load(&Env::from_process())?)
}

/// A tool's runtime: one thread is enough.
pub fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_current_thread().enable_all().build()?)
}

/// A path made absolute against the working folder, as Node's path.resolve made it.
pub fn absolute(path: &std::path::Path) -> std::path::PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| arg.to_string()).collect()
    }

    #[test]
    fn takes_an_options_value_and_a_flag_as_the_ts_tools_did() {
        let given = args(&["--name", "Мама", "--all"]);
        assert_eq!(option(&given, "name"), Some("Мама"));
        assert_eq!(option(&given, "days"), None);
        assert_eq!(option(&given, "all"), None, "a flag at the end has no value");
        assert!(flag(&given, "all"));
        assert!(!flag(&given, "no-asr"));
    }

    #[test]
    fn says_what_is_not_a_command_with_exit_code_2() {
        assert_eq!(command("dance", &[]), 2);
        assert!(run("dance", &[]).unwrap_err().is::<Usage>());
        if cfg!(not(feature = "pc")) {
            let refused = run("call-sim", &[]).unwrap_err();
            assert!(refused.to_string().contains("no calls"), "{refused}");
        }
    }
}
