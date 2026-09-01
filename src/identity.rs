use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TenantId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientInstanceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionKey(pub String);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestIdentity {
    pub tenant_id: TenantId,
    pub device_id: DeviceId,
    pub client_instance_id: ClientInstanceId,
    pub session_key: Option<SessionKey>,
    pub process: ProcessIdentity,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: Option<u32>,
    pub process_started_at: Option<u64>,
    pub parent_pid: Option<u32>,
    pub executable: Option<PathBuf>,
    pub command: Option<String>,
    pub working_directory: Option<PathBuf>,
}

impl RequestIdentity {
    pub fn unknown_local(connection_id: &str) -> Self {
        Self {
            tenant_id: TenantId("local".into()),
            device_id: DeviceId(local_device_id()),
            client_instance_id: ClientInstanceId(format!("connection:{connection_id}")),
            session_key: None,
            process: ProcessIdentity::default(),
        }
    }

    /// Stable routing key priority: trusted session, PID+start time, connection.
    pub fn sticky_key(&self) -> String {
        if let Some(session) = &self.session_key {
            return format!("session:{}", session.0);
        }
        if let (Some(pid), Some(start)) = (self.process.pid, self.process.process_started_at) {
            return format!("process:{pid}:{start}");
        }
        self.client_instance_id.0.clone()
    }
}

pub trait ProcessInspector: Send + Sync {
    fn inspect(&self, peer: SocketAddr, local: SocketAddr) -> ProcessIdentity;
}

/// Safe fallback used when port-to-PID inspection is unavailable or denied.
pub struct UnknownProcessInspector;

impl ProcessInspector for UnknownProcessInspector {
    fn inspect(&self, _peer: SocketAddr, _local: SocketAddr) -> ProcessIdentity {
        ProcessIdentity::default()
    }
}

/// Cross-platform best-effort inspector backed by `netstat2` and `sysinfo`.
/// Any race or permission failure degrades to an empty identity.
pub struct SystemProcessInspector;

impl ProcessInspector for SystemProcessInspector {
    fn inspect(&self, peer: SocketAddr, local: SocketAddr) -> ProcessIdentity {
        use netstat2::{AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, get_sockets_info};
        let family = if peer.is_ipv4() {
            AddressFamilyFlags::IPV4
        } else {
            AddressFamilyFlags::IPV6
        };
        let mut pid = None;
        for attempt in 0..3 {
            if let Ok(sockets) = get_sockets_info(family, ProtocolFlags::TCP) {
                pid = sockets
                    .into_iter()
                    .find_map(|socket| match socket.protocol_socket_info {
                        ProtocolSocketInfo::Tcp(tcp)
                            if tcp.local_port == peer.port() && tcp.remote_port == local.port() =>
                        {
                            socket
                                .associated_pids
                                .into_iter()
                                .find(|candidate| *candidate != std::process::id())
                        }
                        _ => None,
                    });
            }
            if pid.is_some() {
                break;
            }
            if attempt < 2 {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        let Some(pid) = pid else {
            return ProcessIdentity::default();
        };
        let system = sysinfo::System::new_all();
        let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) else {
            return ProcessIdentity {
                pid: Some(pid),
                ..Default::default()
            };
        };
        ProcessIdentity {
            pid: Some(pid),
            process_started_at: Some(process.start_time()),
            parent_pid: process.parent().map(|parent| parent.as_u32()),
            executable: process.exe().map(PathBuf::from),
            // Only retain argv[0]. Later arguments may contain a prompt or
            // workspace data and must never enter monitoring snapshots.
            command: process
                .cmd()
                .first()
                .map(|program| program.to_string_lossy().into_owned()),
            working_directory: process.cwd().map(PathBuf::from),
        }
    }
}

pub(crate) fn local_device_id() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "local-device".into())
}

/// Best-effort stable seed for deriving a non-reversible proxy installation
/// identifier. Raw machine identifiers never leave this process.
pub(crate) fn local_device_seed() -> String {
    #[cfg(unix)]
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(value) = std::fs::read_to_string(path) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_owned();
            }
        }
    }
    local_device_id()
}
