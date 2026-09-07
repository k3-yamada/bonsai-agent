//! ウィンドウ/フォーカス監視センサー (WindowChangedSensor)
//!
//! アクティブなウィンドウ・アプリの切り替えを検知する（フェーズ2）。
//! プライバシー配慮（AppDenylist、PrivacyFilter）を徹底し、パスワードマネージャや銀行アプリ等は
//! イベント発火自体を阻止する。また、クールダウン制御により同一アプリ内のタブ切替等の高頻度ノイズを除去する。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::agent::sensors::sensor::{Sensor, SensorEvent};
use crate::cancel::CancellationToken;
use crate::safety::sensor_filter::{AppDenylist, PrivacyFilter};

/// OS権限状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Granted,
    Denied,
    NotRequested,
}

/// アクセシビリティ権限状態を確認する（環境変数によるテスト用オーバーライド対応）
pub fn check_accessibility_permission() -> PermissionState {
    if let Ok(val) = std::env::var("BONSAI_MOCK_ACCESSIBILITY") {
        match val.to_lowercase().as_str() {
            "granted" | "1" | "true" => return PermissionState::Granted,
            "denied" | "0" | "false" => return PermissionState::Denied,
            _ => return PermissionState::NotRequested,
        }
    }

    #[cfg(target_os = "macos")]
    {
        type AxFn = unsafe extern "C" fn(*const std::ffi::c_void) -> bool;
        unsafe {
            let lib = libc::dlopen(
                c"/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices"
                    .as_ptr(),
                libc::RTLD_LAZY,
            );
            if lib.is_null() {
                return PermissionState::NotRequested;
            }
            let sym = libc::dlsym(lib, c"AXIsProcessTrustedWithOptions".as_ptr());
            if sym.is_null() {
                libc::dlclose(lib);
                return PermissionState::NotRequested;
            }
            let func: AxFn = std::mem::transmute(sym);
            let trusted = func(std::ptr::null());
            libc::dlclose(lib);

            if trusted {
                PermissionState::Granted
            } else {
                PermissionState::Denied
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Granted
    }
}

/// アクティブアプリ・ウィンドウ情報を提供する trait
pub trait ActiveAppProvider: Send + Sync {
    /// 現在最前面のアクティブアプリ名とウィンドウタイトルを返す `(app_name, window_title)`
    fn current_active_app(&self) -> Option<(String, String)>;
}

/// デフォルトの macOS / OS アクティブアプリプロバイダ
pub struct DefaultAppProvider;

impl ActiveAppProvider for DefaultAppProvider {
    fn current_active_app(&self) -> Option<(String, String)> {
        // 必要に応じて macOS システムコールまたは osascript を呼ぶ
        // 常時稼働時は低オーバーヘッドな呼び出しのみ行う
        None
    }
}

/// ウィンドウ/フォーカス監視センサー
pub struct WindowChangedSensor {
    cooldown: Duration,
    denylist: AppDenylist,
    privacy_filter: PrivacyFilter,
    provider: Arc<dyn ActiveAppProvider>,
}

impl WindowChangedSensor {
    pub fn new(cooldown: Duration, denylist: AppDenylist, privacy_filter: PrivacyFilter) -> Self {
        Self {
            cooldown,
            denylist,
            privacy_filter,
            provider: Arc::new(DefaultAppProvider),
        }
    }

    pub fn with_provider(
        cooldown: Duration,
        denylist: AppDenylist,
        privacy_filter: PrivacyFilter,
        provider: Arc<dyn ActiveAppProvider>,
    ) -> Self {
        Self {
            cooldown,
            denylist,
            privacy_filter,
            provider,
        }
    }

    /// 単一のアプリイベントを評価し、発火すべきであれば SensorEvent を返す純粋判定ロジック
    pub fn evaluate_event(
        &self,
        app: &str,
        title: &str,
        last_app: &mut Option<String>,
        last_emit: &mut HashMap<String, Instant>,
        now: Instant,
    ) -> Option<SensorEvent> {
        // 1. AppDenylist による事前遮断 (1Password, Keychain, 銀行等)
        if self.denylist.is_blocked(app) {
            return None;
        }

        // 2. PrivacyFilter によるウィンドウタイトルの機微キーワード遮断
        if !self.privacy_filter.is_window_title_safe(title) {
            return None;
        }

        // 3. アプリ切り替え判定（同一アプリ内のタブ切り替え等はスキップ）
        if let Some(prev) = last_app
            && prev == app
        {
            return None;
        }

        // 4. クールダウン判定（同一アプリへの短時間再スイッチを抑制）
        if let Some(last_time) = last_emit.get(app)
            && now.duration_since(*last_time) < self.cooldown
        {
            return None;
        }

        // 条件クリア: 状態更新とイベント生成
        *last_app = Some(app.to_string());
        last_emit.insert(app.to_string(), now);

        Some(SensorEvent::WindowChanged {
            app: app.to_string(),
            title: title.to_string(),
        })
    }
}

impl Sensor for WindowChangedSensor {
    fn name(&self) -> &'static str {
        "window_focus"
    }

    fn requires_permission(&self) -> bool {
        true
    }

    fn run(&self, tx: mpsc::Sender<SensorEvent>, cancel: CancellationToken) {
        let mut last_app: Option<String> = None;
        let mut last_emit: HashMap<String, Instant> = HashMap::new();

        while !cancel.is_cancelled() {
            if let Some((app, title)) = self.provider.current_active_app()
                && let Some(ev) =
                    self.evaluate_event(&app, &title, &mut last_app, &mut last_emit, Instant::now())
            {
                let _ = tx.send(ev);
            }
            thread::sleep(Duration::from_millis(500));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockAppProvider {
        current: Mutex<Option<(String, String)>>,
    }

    impl MockAppProvider {
        fn new(app: &str, title: &str) -> Self {
            Self {
                current: Mutex::new(Some((app.to_string(), title.to_string()))),
            }
        }

        fn set(&self, app: &str, title: &str) {
            *self.current.lock().unwrap() = Some((app.to_string(), title.to_string()));
        }
    }

    impl ActiveAppProvider for MockAppProvider {
        fn current_active_app(&self) -> Option<(String, String)> {
            self.current.lock().unwrap().clone()
        }
    }

    #[test]
    fn test_window_sensor_requires_permission() {
        let sensor = WindowChangedSensor::new(
            Duration::from_secs(30),
            AppDenylist::default(),
            PrivacyFilter::default(),
        );
        assert!(sensor.requires_permission());
        assert_eq!(sensor.name(), "window_focus");
    }

    #[test]
    fn test_window_sensor_blocks_denylist_apps() {
        let sensor = WindowChangedSensor::new(
            Duration::from_secs(30),
            AppDenylist::default(),
            PrivacyFilter::default(),
        );

        let mut last_app = None;
        let mut last_emit = HashMap::new();
        let now = Instant::now();

        // 1Password は遮断される
        let ev = sensor.evaluate_event("1Password 7", "Vault", &mut last_app, &mut last_emit, now);
        assert_eq!(ev, None);

        // 安全なアプリは通過する
        let ev = sensor.evaluate_event(
            "Terminal",
            "zsh - bonsai",
            &mut last_app,
            &mut last_emit,
            now,
        );
        assert!(matches!(ev, Some(SensorEvent::WindowChanged { .. })));
    }

    #[test]
    fn test_window_sensor_blocks_sensitive_window_title() {
        let sensor = WindowChangedSensor::new(
            Duration::from_secs(30),
            AppDenylist::default(),
            PrivacyFilter::default(),
        );

        let mut last_app = None;
        let mut last_emit = HashMap::new();
        let now = Instant::now();

        // Chrome 自体は安全でも、タイトルが銀行ログインの場合は遮断
        let ev = sensor.evaluate_event(
            "Google Chrome",
            "Online Banking Sign In",
            &mut last_app,
            &mut last_emit,
            now,
        );
        assert_eq!(ev, None);
    }

    #[test]
    fn test_window_sensor_suppresses_same_app_title_changes() {
        let sensor = WindowChangedSensor::new(
            Duration::from_secs(30),
            AppDenylist::default(),
            PrivacyFilter::default(),
        );

        let mut last_app = None;
        let mut last_emit = HashMap::new();
        let now = Instant::now();

        // 初回: VSCode 切り替え発火
        let ev1 = sensor.evaluate_event("Code", "main.rs", &mut last_app, &mut last_emit, now);
        assert!(ev1.is_some());

        // 2回目: 同一アプリ内のタブ切り替え (lib.rs) -> スキップ
        let ev2 = sensor.evaluate_event(
            "Code",
            "lib.rs",
            &mut last_app,
            &mut last_emit,
            now + Duration::from_secs(1),
        );
        assert_eq!(ev2, None, "同一アプリ内のタイトル変化は発火しないこと");
    }

    #[test]
    fn test_window_sensor_respects_cooldown() {
        let sensor = WindowChangedSensor::new(
            Duration::from_secs(30),
            AppDenylist::default(),
            PrivacyFilter::default(),
        );

        let mut last_app = None;
        let mut last_emit = HashMap::new();
        let now = Instant::now();

        // App A に切り替え
        let ev1 = sensor.evaluate_event("AppA", "Title A", &mut last_app, &mut last_emit, now);
        assert!(ev1.is_some());

        // App B に切り替え
        let ev2 = sensor.evaluate_event(
            "AppB",
            "Title B",
            &mut last_app,
            &mut last_emit,
            now + Duration::from_secs(5),
        );
        assert!(ev2.is_some());

        // App A に 10秒後に再切り替え（クールダウン30秒未満） -> スキップ
        let ev3 = sensor.evaluate_event(
            "AppA",
            "Title A",
            &mut last_app,
            &mut last_emit,
            now + Duration::from_secs(10),
        );
        assert_eq!(ev3, None, "クールダウン未満の再スイッチは発火しないこと");

        // App A に 35秒後に再切り替え（クールダウン30秒経過） -> 発火
        let ev4 = sensor.evaluate_event(
            "AppA",
            "Title A",
            &mut last_app,
            &mut last_emit,
            now + Duration::from_secs(35),
        );
        assert!(ev4.is_some(), "クールダウン経過後は発火すること");
    }

    #[test]
    fn test_window_sensor_run_with_mock_provider() {
        let provider = Arc::new(MockAppProvider::new("Terminal", "zsh"));
        let sensor = WindowChangedSensor::with_provider(
            Duration::from_secs(1),
            AppDenylist::default(),
            PrivacyFilter::default(),
            provider.clone(),
        );

        let (tx, rx) = mpsc::channel();
        let cancel = CancellationToken::new();

        let cancel_clone = cancel.clone();
        let handle = thread::spawn(move || {
            sensor.run(tx, cancel_clone);
        });

        // 初回イベント受信
        let ev = rx.recv_timeout(Duration::from_millis(600)).unwrap();
        assert_eq!(
            ev,
            SensorEvent::WindowChanged {
                app: "Terminal".to_string(),
                title: "zsh".to_string(),
            }
        );

        // プロバイダを変更してアプリ切り替え
        provider.set("Firefox", "Rust Docs");
        let ev2 = rx.recv_timeout(Duration::from_millis(600)).unwrap();
        assert_eq!(
            ev2,
            SensorEvent::WindowChanged {
                app: "Firefox".to_string(),
                title: "Rust Docs".to_string(),
            }
        );

        cancel.cancel();
        handle.join().unwrap();
    }
}
