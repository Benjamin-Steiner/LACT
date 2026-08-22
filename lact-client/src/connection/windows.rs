use super::{DaemonConnection, request};
use futures::future::BoxFuture;
use tokio::{
    io::BufReader,
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
};
use tracing::{debug, info, warn};

pub const DEFAULT_PIPE_NAME: &str = r"\\.\pipe\lactd";

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;
const CONNECT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);
const CONNECT_RETRY_ATTEMPTS: usize = 20;

pub struct NamedPipeConnection {
    pipe_name: String,
    inner: BufReader<NamedPipeClient>,
}

impl NamedPipeConnection {
    pub async fn connect(pipe_name: impl Into<String>) -> anyhow::Result<Box<Self>> {
        let pipe_name = pipe_name.into();
        info!(pipe_name = %pipe_name, "Connecting to LACT Windows daemon");

        for attempt in 1..=CONNECT_RETRY_ATTEMPTS {
            match ClientOptions::new().open(&pipe_name) {
                Ok(inner) => {
                    debug!(
                        pipe_name = %pipe_name,
                        attempt,
                        "Connected to LACT Windows daemon named pipe"
                    );
                    return Ok(Box::new(Self {
                        pipe_name,
                        inner: BufReader::new(inner),
                    }));
                }
                // The service may be starting and not have created the first pipe instance yet,
                // or all current pipe instances may be busy. Retry briefly to tolerate that race,
                // but do not wait forever: callers such as the Windows GUI need a real failure so
                // they can start the bundled backend or surface an actionable error to the user.
                Err(err)
                    if matches!(
                        err.raw_os_error(),
                        Some(ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY)
                    ) =>
                {
                    if attempt == CONNECT_RETRY_ATTEMPTS {
                        warn!(
                            pipe_name = %pipe_name,
                            attempt,
                            error = %err,
                            "LACT Windows daemon named pipe did not become available"
                        );
                        return Err(err.into());
                    }
                    tokio::time::sleep(CONNECT_RETRY_DELAY).await;
                }
                Err(err) => {
                    warn!(
                        pipe_name = %pipe_name,
                        attempt,
                        error = %err,
                        "Could not open LACT Windows daemon named pipe"
                    );
                    return Err(err.into());
                }
            }
        }

        unreachable!("named-pipe connection retry loop must return")
    }
}

impl DaemonConnection for NamedPipeConnection {
    fn request<'a>(&'a mut self, payload: &'a str) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async { request(&mut self.inner, payload).await })
    }

    fn new_connection(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn DaemonConnection>>> {
        Box::pin(async move {
            Ok(Self::connect(self.pipe_name.clone()).await? as Box<dyn DaemonConnection>)
        })
    }
}
