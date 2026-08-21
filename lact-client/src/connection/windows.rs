use super::{DaemonConnection, request};
use futures::future::BoxFuture;
use tokio::{
    io::BufReader,
    net::windows::named_pipe::{ClientOptions, NamedPipeClient},
};
use tracing::debug;

pub const DEFAULT_PIPE_NAME: &str = r"\\.\pipe\lactd";

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;
const CONNECT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

pub struct NamedPipeConnection {
    pipe_name: String,
    inner: BufReader<NamedPipeClient>,
}

impl NamedPipeConnection {
    pub async fn connect(pipe_name: impl Into<String>) -> anyhow::Result<Box<Self>> {
        let pipe_name = pipe_name.into();
        debug!("connecting to service at {pipe_name}");

        loop {
            match ClientOptions::new().open(&pipe_name) {
                Ok(inner) => {
                    return Ok(Box::new(Self {
                        pipe_name,
                        inner: BufReader::new(inner),
                    }));
                }
                // The service may be starting and not have created the first pipe instance yet,
                // or all current pipe instances may be busy. Treat both as transient so client
                // startup and daemon/service startup do not race each other.
                Err(err)
                    if matches!(
                        err.raw_os_error(),
                        Some(ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY)
                    ) =>
                {
                    tokio::time::sleep(CONNECT_RETRY_DELAY).await;
                }
                Err(err) => return Err(err.into()),
            }
        }
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
