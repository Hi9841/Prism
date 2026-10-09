//! The backend's async runtime (in place of `tauri::async_runtime`).

use std::future::Future;
use std::sync::LazyLock;

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

static RUNTIME: LazyLock<Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .thread_name("prism-core")
        .build()
        .expect("failed to start the prism-core runtime")
});

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    RUNTIME.spawn(future)
}

pub fn spawn_blocking<F, R>(work: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    RUNTIME.spawn_blocking(work)
}

/// Runs a future to completion on the calling thread (never a runtime thread).
pub fn block_on<F: Future>(future: F) -> F::Output {
    RUNTIME.block_on(future)
}
