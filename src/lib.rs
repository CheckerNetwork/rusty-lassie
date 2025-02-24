use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

mod start_error;

pub use start_error::StartError;

#[cfg_attr(
    all(target_os = "windows", target_env = "msvc"),
    link(name = "golassie.dll")
)]
#[cfg_attr(
    not(all(target_os = "windows", target_env = "msvc")),
    link(name = "golassie")
)]
extern "C" {
    fn InitDaemon(config: *const GoDaemonConfig) -> InitDaemonResult;
    fn DropDaemonInitResult(result: *mut InitDaemonResult);
    fn RunDaemon() -> LassieResult;
    fn StopDaemon() -> LassieResult;
    fn DropResult(value: *mut LassieResult);
}

#[repr(C)]
#[derive(Debug)]
struct InitDaemonResult {
    port: u16,
    error: *const c_char,
}

impl Drop for InitDaemonResult {
    fn drop(&mut self) {
        // SAFETY:
        // We can safely call the FFI function to free the memory used by InitDaemonResult, because
        // Rust guarantees that the `drop` function is called only once for each InitDaemonResult
        // instance. Also InitDaemonResult is a private struct that's visible only inside this file,
        // and we are never instantiate it directly, we always obtain instances via FFI calls.
        unsafe { DropDaemonInitResult(self) }
    }
}

impl InitDaemonResult {
    fn error(&self) -> Option<String> {
        from_c_string(self.error)
    }
}

#[repr(C)]
#[derive(Debug)]
struct LassieResult {
    error: *const c_char,
}

impl Drop for LassieResult {
    fn drop(&mut self) {
        // SAFETY:
        // We can safely call the FFI function to free the memory used by LassieResult, because Rust
        // guarantees that the `drop` function is called only once for each LassieResult instance.
        // Also LassieResult is a private struct that's visible only inside this file, and we are
        // never instantiate it directly, we always obtain instances via FFI calls.
        unsafe { DropResult(self) }
    }
}

impl LassieResult {
    fn error(&self) -> Option<String> {
        from_c_string(self.error)
    }
}

fn from_c_string(str: *const c_char) -> Option<String> {
    if str.is_null() {
        return None;
    }

    // SAFETY:
    // We already checked that str is not NULL, see above.
    Some(unsafe { CStr::from_ptr(str) }.to_string_lossy().to_string())
}

#[repr(C)]
struct GoDaemonConfig {
    // this must be kept in sync with the definition of daemon_config_t in go-lib/lassie-ffi.go
    temp_dir: *const c_char,
    port: u16,
    log_level: usize,
    max_blocks: u64,
    provider_timeout: i64,
    global_timeout: i64,
    access_token: *const c_char,
    lassie_user_agent: *const c_char,
}

struct GoDaemon {
    handler_thread: std::thread::JoinHandle<()>,
}

static mut DAEMON: Mutex<Option<GoDaemon>> = Mutex::new(None);

#[allow(static_mut_refs)]
fn get_global_daemon() -> std::sync::LockResult<MutexGuard<'static, Option<GoDaemon>>> {
    // SAFETY:
    // We are accessing the global variable from this place only and it's protected by a Mutex.
    unsafe { DAEMON.lock() }
}

#[derive(Debug, Clone, Default)]
pub struct DaemonConfig {
    /// Directory where to store temporary files (CAR store).
    ///
    /// By default, Lassie stores temporary files in the OS-specific temp directory.
    pub temp_dir: Option<PathBuf>,

    /// Port where to listen.
    ///
    /// By default, we ask the operating system to choose a free ephemeral port.
    pub port: u16,

    /// `MaxBlocks` optionally specifies the maximum number of blocks to fetch.
    ///
    /// When the requested CID contains more blocks than specified, the HTTP response will be
    /// aborted in a way that triggers a client error.
    pub max_blocks: Option<u64>,

    /// Specify a custom timeout for retrieving data from a provider. Beyond this limit, when no
    /// data has been received, the retrieval will fail.
    ///
    /// At the moment, this configuration applies to Bitswap retrievals only and controls how
    /// much time we allow for the storage provider to send us the next block.
    ///
    /// On timeout, the HTTP response will be aborted in a way that triggers a client error.
    ///
    /// The default timeout is controlled by Go version of Lassie and you should not rely on any
    /// particular value. Provide your own value if this timeout is important for you.
    pub provider_timeout: Option<Duration>,

    /// Specify a custom timeout for the entire retrieval process.
    ///
    /// On timeout, the HTTP response will be aborted in a way that triggers a client error.
    ///
    /// No timeout is enforced by default.
    pub global_timeout: Option<Duration>,

    /// Require retrieval requests to provide authorization header with the configured access token.
    ///
    /// For example: `Authorization: Bearer {token}`
    pub access_token: Option<String>,
}

pub struct Daemon {
    port: u16,
    access_token: Option<String>,
}

impl Daemon {
    /// # Errors
    ///
    /// This function returns `Err` when you are trying to start more than instance, the configured
    /// `temp_dir` path cannot be converted to a Go string, or Lassie cannot start the HTTP server.
    pub fn start(config: DaemonConfig) -> Result<Self, StartError> {
        log::debug!("[Daemon::start] Locking global daemon mutex");
        let mut maybe_daemon = get_global_daemon().map_err(|_| StartError::MutexPoisoned)?;
        if maybe_daemon.is_some() {
            log::error!("{}", StartError::OnlyOneInstanceAllowed);
            return Err(StartError::OnlyOneInstanceAllowed);
        }

        log::info!("Starting Lassie Daemon");
        let temp_dir = match config.temp_dir {
            None => String::new(),
            Some(dir) => {
                let str = dir.to_str();
                match str {
                    None => return Err(StartError::PathIsNotValidUtf8(dir)),
                    Some(val) => val.to_string(),
                }
            }
        };

        let temp_dir = CString::new(temp_dir.clone())
            .map_err(|_| StartError::PathContainsNullByte(temp_dir))?;

        let log_level = if log::log_enabled!(log::Level::Debug) {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Off
        };

        let global_timeout = match config.global_timeout {
            Some(d) => try_convert_duration_to_go_type(d)?,
            None => 0,
        };

        let provider_timeout = match config.provider_timeout {
            Some(d) => try_convert_duration_to_go_type(d)?,
            None => 0,
        };

        let access_token = config.access_token.clone().unwrap_or_default();
        let access_token = CString::new(access_token.clone())
            .map_err(|_| StartError::AccessTokenContainsNullByte(access_token.to_string()))?;

        // See https://github.com/filecoin-project/lassie/pull/240
        let lassie_version = env!("LASSIE_VERSION");
        let lassie_user_agent = format!("lassie/v{lassie_version}");
        let lassie_user_agent = CString::new(lassie_user_agent.clone()).map_err(|_| {
            StartError::Lassie("Internal error: invalid Lassie version.".to_string())
        })?;

        let go_config = GoDaemonConfig {
            temp_dir: temp_dir.as_ptr(),
            log_level: log_level as usize,
            port: config.port,
            global_timeout,
            provider_timeout,
            max_blocks: config.max_blocks.unwrap_or(0),
            access_token: access_token.as_ptr(),
            lassie_user_agent: lassie_user_agent.as_ptr(),
        };

        // SAFETY:
        // It's safe to call this FFI function as it does not have any special safety requirements
        // and we know that `&go_config` is not a NULL pointer.
        let result = unsafe { InitDaemon(&go_config) };
        log::debug!("Lassie.InitDaemon result: {:?}", result);

        if let Some(msg) = result.error() {
            log::error!("Lassie.InitDaemon failed: {msg}");
            return Err(StartError::Lassie(msg));
        }
        let port = result.port;
        log::debug!("Lassie.InitDaemon returned port: {port}");

        let handler_thread = std::thread::spawn(|| {
            log::debug!("Running Lassie HTTP handler");
            // SAFETY:
            // This FFI function is designed to be called from a different thread.
            let result = unsafe { RunDaemon() };
            if let Some(msg) = result.error() {
                log::error!("Lassie HTTP handler failed: {msg}");
                // TODO: should we somehow notify the main thread about the problem?
                // Maybe we should panic? That would not kill the main thread though.
            }
            log::debug!("HTTP handler exited");
        });
        *maybe_daemon = Some(GoDaemon { handler_thread });

        log::info!("Lassie Daemon is listening on port {}", port);
        Ok(Daemon {
            port,
            access_token: config.access_token,
        })
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[must_use]
    pub fn access_token(&self) -> &Option<String> {
        &self.access_token
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        log::debug!("[Daemon::drop] Locking global daemon mutex");
        let mut maybe_daemon = get_global_daemon().expect("global daemon mutex was poisoned");
        assert!(
            maybe_daemon.is_some(),
            "Daemon.drop() was called when no GoDaemon was running"
        );

        log::debug!("Shutting down Lassie Daemon");
        // SAFETY:
        // We can call this FFI function as it does not have any special safety requirements.
        let result = unsafe { StopDaemon() };
        if let Some(msg) = result.error() {
            panic!("Cannot stop Lassie Daemon: {msg}");
        }

        log::debug!("Waiting for Lassie to exit");
        // It's safe to call unwrap() here because we already handled maybe_daemon.is_none() above
        let GoDaemon { handler_thread } = maybe_daemon.take().unwrap();
        handler_thread.join().expect("Lassie handler panicked");
    }
}

fn try_convert_duration_to_go_type(from: Duration) -> Result<i64, StartError> {
    // Go Duration type represents the elapsed time between two instants as an int64 nanosecond count.
    i64::try_from(from.as_nanos()).map_err(|_| StartError::DurationIsTooLong(from))
}

#[cfg(test)]
mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    // Rust runs tests in parallel. Since Lassie Daemon is a singleton,
    // we must synchronise the tests to ensure they run sequentially
    static TEST_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn can_start_after_stopping() {
        let _lock = setup_test_env();
        let d = Daemon::start(DaemonConfig::default()).expect("cannot start the first time");
        drop(d);
        let _ = Daemon::start(DaemonConfig::default()).expect("cannot start the second time");
    }

    #[test]
    fn cannot_start_twice() {
        let _lock = setup_test_env();
        let _first =
            Daemon::start(DaemonConfig::default()).expect("cannot start the first instance");
        match Daemon::start(DaemonConfig::default()) {
            Ok(_) => panic!("starting another instance should have failed"),

            Err(err) => assert_eq!(err, StartError::OnlyOneInstanceAllowed),
        };
    }

    #[test]
    #[cfg_attr(windows, ignore)]
    fn reports_listen_error() {
        let _lock = setup_test_env();
        let result = Daemon::start(DaemonConfig {
            port: 1,
            ..DaemonConfig::default()
        });
        match result {
            Ok(_) => panic!("starting Lassie on port 1 should have failed"),
            Err(StartError::Lassie(msg)) => {
                assert!(
                    msg.contains("cannot start the HTTP server")
                        && msg.contains("listen tcp 127.0.0.1:1")
                        && msg.contains("permission denied"),
                    "Expected bind-socket permission error, actual: {msg}",
                );
            }
            Err(err) => panic!("unexpected error while starting Lassie on port 1: {err}"),
        };
    }

    #[test]
    fn start_returns_access_token() {
        let token = Some("super_secret".to_string());
        let _lock = setup_test_env();
        let result = Daemon::start(DaemonConfig {
            access_token: token.clone(),
            ..DaemonConfig::default()
        })
        .expect("cannot start Lassie daemon");

        assert_eq!(*result.access_token(), token);
    }

    fn setup_test_env() -> MutexGuard<'static, ()> {
        let _ = env_logger::builder().is_test(true).try_init();
        let lock = TEST_GUARD.lock().expect("cannot obtain global test lock. This typically happens when one of the test fails; the problem should go away after you fix the test failure.");
        lock
    }
}
