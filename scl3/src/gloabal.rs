use std::sync::{Arc, RwLock};

#[derive(Debug, Default)]
pub struct AppState {
    pub request_count: u64,
    pub active_users: Vec<String>,
}

pub fn state() -> &'static RwLock<AppState> {
    static STATE: OnceLock<RwLock<AppState>> = OnceLock::new();
    STATE.get_or_init(|| RwLock::new(AppState::default()))
}

// 使用
fn handle_request() {
    // 读
    let count = state().read().unwrap().request_count;
    
    // 写
    state().write().unwrap().request_count += 1;
}