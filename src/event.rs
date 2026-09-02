use std::sync::OnceLock;
use tokio::sync::watch;

static OUTBOUND_MODE: OnceLock<watch::Sender<String>> = OnceLock::new();
static SYSTEM_PROXY: OnceLock<watch::Sender<bool>> = OnceLock::new();
static TUN_PROXY: OnceLock<watch::Sender<bool>> = OnceLock::new();
static CORE_STATE: OnceLock<watch::Sender<CoreState>> = OnceLock::new();
static TUN_CONFIRMATION_REQUESTS: OnceLock<watch::Sender<()>> = OnceLock::new();

/// 核心生命周期状态及会话代次。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct CoreState {
    pub(crate) running: bool,
    pub(crate) generation: u64,
}

fn outbound_mode_sender() -> &'static watch::Sender<String> {
    OUTBOUND_MODE.get_or_init(|| watch::channel(String::new()).0)
}

fn system_proxy_sender() -> &'static watch::Sender<bool> {
    SYSTEM_PROXY.get_or_init(|| watch::channel(false).0)
}

fn tun_proxy_sender() -> &'static watch::Sender<bool> {
    TUN_PROXY.get_or_init(|| watch::channel(false).0)
}

fn core_state_sender() -> &'static watch::Sender<CoreState> {
    CORE_STATE.get_or_init(|| watch::channel(CoreState::default()).0)
}

fn tun_confirmation_requests_sender() -> &'static watch::Sender<()> {
    TUN_CONFIRMATION_REQUESTS.get_or_init(|| watch::channel(()).0)
}

/// 订阅出站模式状态。
pub(crate) fn subscribe_outbound_mode() -> watch::Receiver<String> {
    outbound_mode_sender().subscribe()
}

/// 订阅系统代理状态。
pub(crate) fn subscribe_system_proxy() -> watch::Receiver<bool> {
    system_proxy_sender().subscribe()
}

/// 订阅 TUN 代理状态。
pub(crate) fn subscribe_tun_proxy() -> watch::Receiver<bool> {
    tun_proxy_sender().subscribe()
}

/// 订阅核心生命周期状态。
pub(crate) fn subscribe_core_state() -> watch::Receiver<CoreState> {
    core_state_sender().subscribe()
}

/// 订阅 TUN 管理员确认请求。
pub(crate) fn subscribe_tun_confirmation_requests() -> watch::Receiver<()> {
    tun_confirmation_requests_sender().subscribe()
}

/// 发布出站模式变化事件。
pub(crate) fn publish_outbound_mode(mode: &str) {
    outbound_mode_sender().send_replace(mode.to_string());
}

/// 发布系统代理状态。
pub(crate) fn publish_system_proxy(enabled: bool) {
    system_proxy_sender().send_replace(enabled);
}

/// 发布 TUN 代理状态。
pub(crate) fn publish_tun_proxy(enabled: bool) {
    tun_proxy_sender().send_replace(enabled);
}

/// 发布核心生命周期状态。
pub(crate) fn publish_core_state(running: bool, generation: u64) {
    core_state_sender().send_replace(CoreState {
        running,
        generation,
    });
}

/// 发布 TUN 管理员确认请求。
pub(crate) fn request_tun_confirmation() {
    tun_confirmation_requests_sender().send_replace(());
}

#[cfg(test)]
mod tests {
    use super::{core_state_sender, CoreState};
    use tokio::sync::watch;

    #[test]
    fn core_state_watch_delivers_lifecycle_states() {
        let sender = core_state_sender();
        let mut receiver = sender.subscribe();

        sender.send_replace(CoreState {
            running: true,
            generation: 1,
        });
        assert!(receiver.has_changed().expect("核心运行状态通道不应关闭"));
        assert_eq!(
            *receiver.borrow_and_update(),
            CoreState {
                running: true,
                generation: 1,
            }
        );

        sender.send_replace(CoreState {
            running: false,
            generation: 2,
        });
        assert!(receiver.has_changed().expect("核心运行状态通道不应关闭"));
        assert_eq!(
            *receiver.borrow_and_update(),
            CoreState {
                running: false,
                generation: 2,
            }
        );
    }

    #[test]
    fn core_state_generation_change_is_observable_when_running_is_unchanged() {
        let (sender, mut receiver) = watch::channel(CoreState {
            running: true,
            generation: 4,
        });
        sender.send_replace(CoreState {
            running: true,
            generation: 5,
        });

        assert!(receiver.has_changed().expect("核心状态通道不应关闭"));
        assert_eq!(receiver.borrow_and_update().generation, 5);
    }

    #[test]
    fn late_watch_subscription_can_process_current_value() {
        let (sender, mut receiver) = watch::channel(CoreState::default());
        sender.send_replace(CoreState {
            running: true,
            generation: 6,
        });
        receiver.mark_changed();

        assert!(receiver.has_changed().expect("核心运行状态通道不应关闭"));
        assert_eq!(receiver.borrow_and_update().generation, 6);
    }

    #[test]
    fn rapid_stop_start_updates_keep_latest_generation() {
        let (sender, mut receiver) = watch::channel(CoreState {
            running: true,
            generation: 7,
        });
        sender.send_replace(CoreState {
            running: false,
            generation: 8,
        });
        sender.send_replace(CoreState {
            running: true,
            generation: 9,
        });

        assert!(receiver.has_changed().expect("核心状态通道不应关闭"));
        assert_eq!(
            *receiver.borrow_and_update(),
            CoreState {
                running: true,
                generation: 9,
            }
        );
    }
}
