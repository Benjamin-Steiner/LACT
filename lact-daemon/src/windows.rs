use anyhow::Context;
use lact_schema::{DeviceListEntry, Pong, Request, Response, SystemInfo, VersionInfo};
use serde::Serialize;
use std::{env, fmt::Debug};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::windows::named_pipe::ServerOptions,
    runtime,
};
use tracing::{error, info, trace};

const DEFAULT_PIPE_NAME: &str = r"\\.\pipe\lactd";

pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt().init();

    let rt = runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Could not initialize Tokio runtime")?;

    rt.block_on(run_server())
}

async fn run_server() -> anyhow::Result<()> {
    let pipe_name = env::var("LACT_DAEMON_PIPE").unwrap_or_else(|_| DEFAULT_PIPE_NAME.to_owned());
    info!("Windows daemon listening on {pipe_name}");

    let mut first_instance = true;
    loop {
        let mut options = ServerOptions::new();
        if first_instance {
            options.first_pipe_instance(true);
        }

        let server = options
            .create(&pipe_name)
            .with_context(|| format!("Could not create named pipe {pipe_name}"))?;
        first_instance = false;

        server.connect().await.context("Named pipe connect failed")?;
        info!("Windows client connected");

        if let Err(err) = handle_stream(server).await {
            error!("Windows client connection failed: {err:#}");
        }
    }
}

async fn handle_stream<T>(stream: T) -> anyhow::Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut stream = BufReader::new(stream);
    let mut buf = String::new();

    while stream.read_line(&mut buf).await? != 0 {
        trace!("handling Windows request: {}", buf.trim_end());

        let response = match serde_json::from_str::<Request<'_>>(&buf) {
            Ok(Request::Ping) => ok_response(Pong)?,
            Ok(Request::SystemInfo) => ok_response(system_info())?,
            Ok(Request::ListDevices) => ok_response(Vec::<DeviceListEntry>::new())?,
            Ok(_) => serde_json::to_vec(&Response::<()>::from(anyhow::anyhow!(
                "This request is not implemented by the Windows backend yet"
            )))?,
            Err(err) => serde_json::to_vec(&Response::<()>::from(
                anyhow::Error::new(err).context("Failed to deserialize request"),
            ))?,
        };

        stream.write_all(&response).await?;
        stream.write_all(b"\n").await?;
        buf.clear();
    }

    Ok(())
}

fn system_info() -> SystemInfo {
    SystemInfo {
        version: VersionInfo::current(),
        distro: Some("Windows".to_owned()),
        kernel_version: env::var("OS").unwrap_or_else(|_| "Windows".to_owned()),
        amdgpu_overdrive_enabled: None,
        amdgpu_params_configurator: None,
    }
}

fn ok_response<T: Serialize + Debug>(data: T) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&Response::Ok(data))?)
}
