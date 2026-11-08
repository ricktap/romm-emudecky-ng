//! Process entry point of the native backend.
//!
//! `romm-emudecky-ng-backend --list-methods` prints the callable names as a
//! JSON array and exits (the Decky shim reads it at import time).
//! `romm-emudecky-ng-backend --socket <path>` runs the backend: startup work,
//! then the protocol server on `<path>` until `__shutdown__` or SIGTERM.
//! The Decky directories come from the `DECKY_*` environment the shim passes.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use romm_backend::adapters::persistence::system_clock;
use romm_backend::adapters::romm::http::RetryPolicy;
use romm_backend::app::{Backend, BackendEnv, callable_names};
use romm_backend::rpc::server::{EventHub, Server, ShutdownHook};

/// Test seam: overrides the retry backoff base (milliseconds).
const RETRY_BASE_ENV: &str = "ROMM_BACKEND_RETRY_BASE_MS";
const SOCKET_ENV: &str = "ROMM_BACKEND_SOCKET";
const LOG_FILE: &str = "backend.log";

enum Command {
    ListMethods,
    Serve(PathBuf),
}

fn parse_args() -> anyhow::Result<Command> {
    let mut args = std::env::args().skip(1);
    let mut socket = std::env::var_os(SOCKET_ENV).map(PathBuf::from);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list-methods" => return Ok(Command::ListMethods),
            "--socket" => socket = Some(args.next().context("--socket needs a path")?.into()),
            other => bail!("unknown argument: {other}"),
        }
    }
    match socket {
        Some(path) => Ok(Command::Serve(path)),
        None => bail!("usage: romm-emudecky-ng-backend --socket <path> | --list-methods"),
    }
}

fn env_dir(name: &str) -> anyhow::Result<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .with_context(|| format!("{name} is not set"))
}

fn init_logging() {
    let builder = tracing_subscriber::fmt()
        .with_target(false)
        .with_max_level(tracing::Level::INFO);
    let file = std::env::var_os("DECKY_PLUGIN_LOG_DIR").and_then(|dir| {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::File::create(dir.join(LOG_FILE)).ok()
    });
    match file {
        Some(file) => builder.with_ansi(false).with_writer(std::sync::Mutex::new(file)).init(),
        None => builder.with_writer(std::io::stderr).init(),
    }
}

/// Ask the kernel to SIGTERM us if the plugin process dies without a shutdown.
fn die_with_parent() {
    #[cfg(target_os = "linux")]
    // SAFETY: prctl(PR_SET_PDEATHSIG) takes plain integers and has no memory effects.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }
}

async fn serve(socket_path: PathBuf) -> anyhow::Result<()> {
    let retry = match std::env::var(RETRY_BASE_ENV).ok().and_then(|v| v.parse::<u64>().ok()) {
        Some(ms) => RetryPolicy {
            base_delay: Duration::from_millis(ms),
            ..RetryPolicy::default()
        },
        None => RetryPolicy::default(),
    };
    let user_home = std::env::var_os("DECKY_USER_HOME")
        .or_else(|| std::env::var_os("HOME"))
        .unwrap_or_default();
    let env = BackendEnv {
        settings_dir: env_dir("DECKY_PLUGIN_SETTINGS_DIR")?,
        runtime_dir: env_dir("DECKY_PLUGIN_RUNTIME_DIR")?,
        plugin_dir: env_dir("DECKY_PLUGIN_DIR")?,
        user_home: user_home.into(),
        retry,
        clock: system_clock(),
    };
    let hub = Arc::new(EventHub::default());
    let backend = Backend::start(env, hub.clone()).await?;
    let hook: ShutdownHook = Arc::new(|| {
        Box::pin(async {
            tracing::info!("EmuDecky RomM Manager backend stopped");
            Ok(())
        })
    });
    let server = Server::new(backend.registry(), hub, Some(hook));
    let listener = Server::bind(&socket_path).with_context(|| format!("binding {}", socket_path.display()))?;
    tracing::info!("EmuDecky RomM Manager backend started on {}", socket_path.display());

    let stopper = server.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        tracing::info!("signal received; shutting down");
        stopper.stop();
    });
    server.serve(listener, socket_path).await;
    Ok(())
}

fn main() -> ExitCode {
    let command = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e:#}");
            return ExitCode::from(2);
        }
    };
    match command {
        Command::ListMethods => {
            println!("{}", serde_json::Value::from(callable_names()));
            ExitCode::SUCCESS
        }
        Command::Serve(socket_path) => {
            die_with_parent();
            init_logging();
            let runtime = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    tracing::error!("cannot start the async runtime: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match runtime.block_on(serve(socket_path)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    tracing::error!("backend failed to start: {e:#}");
                    eprintln!("backend failed to start: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
