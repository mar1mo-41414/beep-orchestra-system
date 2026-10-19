//! HTTP ハンドラ間で共有する状態。

use beep_common::node::NodeInfo;
use std::sync::Arc;

use crate::config::DaemonConfig;
use crate::engine::Engine;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<DaemonConfig>,
    pub self_info: Arc<NodeInfo>,
    pub engine: Engine,
    /// LAN 上で発見したノード (Phase3 で実装)。
    pub peers: Arc<parking_lot_ish::PeerTable>,
}

/// 発見済みピアの保管所。observed_at で TTL 管理する。
pub mod parking_lot_ish {
    use beep_common::node::NodeInfo;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    struct Entry {
        info: NodeInfo,
        observed_at: Instant,
    }

    #[derive(Default)]
    pub struct PeerTable {
        inner: Mutex<HashMap<Uuid, Entry>>,
    }

    impl PeerTable {
        pub fn snapshot(&self) -> Vec<NodeInfo> {
            self.inner
                .lock()
                .unwrap()
                .values()
                .map(|e| e.info.clone())
                .collect()
        }

        pub fn upsert(&self, info: NodeInfo) {
            let mut g = self.inner.lock().unwrap();
            g.insert(
                info.uuid,
                Entry {
                    info,
                    observed_at: Instant::now(),
                },
            );
        }

        /// TTL 超過のエントリを削除。
        pub fn gc_older_than(&self, ttl: Duration) {
            let now = Instant::now();
            let mut g = self.inner.lock().unwrap();
            g.retain(|_, e| now.duration_since(e.observed_at) <= ttl);
        }
    }
}
