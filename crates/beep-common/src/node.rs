//! LAN 上のノード情報。Phase3 以降で本格利用する。

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use uuid::Uuid;

/// LAN 上で発見/告知するノード。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub uuid: Uuid,
    pub name: String,
    pub ip: IpAddr,
    pub http_port: u16,
    /// 「機能」フラグ。将来 beep 以外のバックエンドが増えたときに拡張。
    pub capabilities: Vec<String>,
}

impl NodeInfo {
    pub fn new(name: impl Into<String>, ip: IpAddr, http_port: u16) -> Self {
        Self {
            uuid: Uuid::new_v4(),
            name: name.into(),
            ip,
            http_port,
            capabilities: vec!["beep".into()],
        }
    }
}
