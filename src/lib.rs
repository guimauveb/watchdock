//! Tool acting like systemd's Watchdog for Docker containers.
//! Signals that the Docker container should restart if `WatchDock::notify` was not called within a certain period.
use {
    log::{error, info},
    shiplift::{Container, ContainerListOptions, Docker},
    std::time::Duration,
    thiserror::Error as ThisError,
    tokio::{
        sync::mpsc::{unbounded_channel, UnboundedSender},
        task::{spawn, JoinError},
        time::timeout,
    },
};

#[derive(Debug, ThisError)]
pub enum Error {
    #[error("Docker error")]
    Docker(#[from] shiplift::Error),
    #[error("Container not found {0}")]
    ContainerNotFound(String),
    #[error("Join error")]
    Join(#[from] JoinError),
    #[error("Send error")]
    Send,
}

/// Tool acting like systemd's Watchdog for Docker containers.
/// Signals that the Docker container should restart if `WatchDock::notify` was not called within a certain period.
pub struct WatchDock {
    sender: UnboundedSender<()>,
}

impl WatchDock {
    /// Get Docker container.
    async fn get_container<'docker>(
        docker: &'docker Docker,
        container_id: &str,
    ) -> Result<Container<'docker>, Error> {
        let mut list_builder = ContainerListOptions::builder();
        list_builder.all();
        let options = list_builder.build();
        docker
            .containers()
            .list(&options)
            .await?
            .iter()
            .find(|&container| container.id.starts_with(container_id))
            .map_or_else(
                || Err(Error::ContainerNotFound(container_id.into())),
                |container| Ok(docker.containers().get(&container.id)),
            )
    }

    /// Notify that the container should keep running.
    pub fn notify(&self) -> Result<(), Error> {
        self.sender.send(()).map_err(|_| Error::Send)
    }

    /// Create a new `WatchDock` instance where `max_time` is the maximum time to wait before restarting the given `container_name`
    /// if `WatchDock::notify` was not called during that time.
    ///
    /// To access the Docker API from the container, the Docker socket path must be passed as a shared volume
    /// when building the container, e.g:
    /// `$ docker run -v /var/run/docker.sock:/var/run/docker.sock container:latest`
    pub async fn new<I>(max_time: Duration, container_id: I) -> Result<Self, Error>
    where
        I: AsRef<str> + Send + Sync + 'static,
    {
        let (sender, mut receiver) = unbounded_channel();
        spawn(async move {
            let docker = Docker::new();
            if let Ok(container) = Self::get_container(&docker, container_id.as_ref()).await {
                loop {
                    if timeout(max_time, receiver.recv()).await.is_err() {
                        info!("Timeout reached. Restarting container...");
                        container
                            .restart(None)
                            .await
                            .expect("Cannot restart container");
                    }
                }
            } else {
                error!(
                    "Cannot find container: {:?}. Terminating WatchDock.",
                    container_id.as_ref()
                );
            }
        });

        Ok(Self { sender })
    }
}
