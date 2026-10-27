//! `magnum-bi desktop`: lite (lite.rs) in a window of its own, through the system's web engine
//! (WebView2 on Windows, WebKit on macOS, WebKitGTK on Linux). Built only with `--features desktop`:
//! the server's build never links a GUI. The window shows the local app only: other addresses open
//! in the person's browser, downloads land in their Downloads folder, and closing it stops lite.

use std::path::{Path, PathBuf};

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::{Icon, Window, WindowBuilder};
use wry::{NewWindowResponse, WebContext, WebView, WebViewBuilder};

/// What the server thread tells the window.
enum Up {
    /// lite is listening at this address
    Ready(String),
    /// it couldn't start, or stopped with this error
    Failed(String),
    /// it stopped (asked to from outside: Ctrl+C, logging out): the window goes too
    Stopped,
}

/// The window's icon (packaging/icon.svg at 64x64, as RGBA).
const ICON: &[u8] = include_bytes!("../../packaging/icon-64.rgba");

const STARTING: &str = r#"<!doctype html><meta charset="utf-8"><title>Magnum BI</title>
<body style="font:16px system-ui,sans-serif;display:grid;place-items:center;height:90vh;color:#444">
<div style="text-align:center"><h1 style="font-weight:600">Magnum BI</h1>
<p>Starting… The first time, it also downloads its PostgreSQL (about 30 MB).</p></div>"#;

/// `magnum-bi desktop`: lite, in a window, until the window closes.
pub fn run(dir: PathBuf) -> anyhow::Result<()> {
    // no GTK application id: GTK hands a second launch to the first one, which tao doesn't take;
    // Linux matches the window to its menu entry (magnum-bi.desktop) by the program's name
    let event_loop = EventLoopBuilder::<Up>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    // lite, on its own thread and runtime: the window has the main thread. Opened twice, the
    // second window only says where the first one is.
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let started = crate::lite::lock(&dir).and_then(|one| Ok((one, crate::lite::port(&dir, None)?)));
    let port = started.as_ref().map_or(0, |(_, port)| *port);
    let mut server = match started {
        Err(e) => {
            let _ = proxy.send_event(Up::Failed(format!("{e:#}")));
            None
        }
        Ok((one, port)) => {
            let server_dir = dir.clone();
            Some(std::thread::spawn(move || {
                let _one = one;
                let told = proxy.clone();
                let ready = move |address: &str| {
                    let _ = told.send_event(Up::Ready(address.to_string()));
                };
                // the window closing, or the program being asked to stop from outside
                let asked = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let asked_now = asked.clone();
                let until = async move {
                    tokio::select! {
                        _ = stopped => {}
                        _ = crate::shutdown() => {}
                    }
                    asked_now.store(true, std::sync::atomic::Ordering::SeqCst);
                };
                let result = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .map_err(anyhow::Error::from)
                    .and_then(|rt| rt.block_on(crate::lite(Some(port), server_dir, ready, until)));
                // asked to stop, it goes, whatever went wrong on the way out (a failure page
                // would keep a window nobody asked for)
                let asked = asked.load(std::sync::atomic::Ordering::SeqCst);
                let _ = proxy.send_event(match result {
                    Err(e) if !asked => Up::Failed(format!("{e:#}")),
                    Err(e) => {
                        tracing::warn!("stopping Magnum BI lite: {e:#}");
                        Up::Stopped
                    }
                    Ok(()) => Up::Stopped,
                });
            }))
        }
    };

    let window = WindowBuilder::new()
        .with_title("Magnum BI")
        .with_inner_size(LogicalSize::new(1280.0, 860.0))
        .with_min_inner_size(LogicalSize::new(640.0, 480.0))
        .with_window_icon(Icon::from_rgba(ICON.to_vec(), 64, 64).ok())
        .build(&event_loop)?;
    // its storage (preferences, an unlocked PIN) in the lite folder, like everything else
    let mut context = WebContext::new(Some(dir.join("window")));
    let downloads = std::env::home_dir().map(|h| h.join("Downloads")).unwrap_or_else(|| dir.clone());
    let builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_html(STARTING)
        .with_navigation_handler(move |url| {
            // the app's own pages, its downloads (blob:) and the starting page; anything else
            // opens in the person's browser
            if is_app(&url, port) || url.starts_with("blob:") || url == "about:blank" {
                return true;
            }
            if opens_outside(&url) {
                crate::lite::open_browser(&url);
            }
            false
        })
        .with_new_window_req_handler(|url, _| {
            if opens_outside(&url) {
                crate::lite::open_browser(&url);
            }
            NewWindowResponse::Deny
        })
        .with_download_started_handler(move |_, path| {
            *path = download_path(path, &downloads);
            true
        });
    let webview = attach(builder, &window)?;

    let mut stop = Some(stop);
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(Up::Ready(address)) => {
                let _ = webview.load_url(&address);
            }
            Event::UserEvent(Up::Failed(error)) => {
                let shown = error.replace('&', "&amp;").replace('<', "&lt;");
                let page = format!(
                    "<!doctype html><meta charset=\"utf-8\"><body style=\"font:15px system-ui,sans-serif;padding:2em\">\
                     <h2>Magnum BI couldn't start</h2><pre style=\"white-space:pre-wrap\">{shown}</pre>"
                );
                let _ = webview.load_html(&page);
            }
            Event::UserEvent(Up::Stopped) => {
                if let Some(server) = server.take() {
                    let _ = server.join();
                }
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                // closed: lite and its PostgreSQL stop, then the program
                window.set_visible(false);
                if let Some(stop) = stop.take() {
                    let _ = stop.send(());
                }
                if let Some(server) = server.take() {
                    let _ = server.join();
                }
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

/// The web view, in the window (Linux: inside its GTK box).
fn attach(builder: WebViewBuilder, window: &Window) -> anyhow::Result<WebView> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let webview = builder.build(window)?;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window.default_vbox().ok_or_else(|| anyhow::anyhow!("the window has no GTK box"))?;
        builder.build_gtk(vbox)?
    };
    Ok(webview)
}

/// Whether the window may show `url` itself: the local app on its own port, nothing else.
pub fn is_app(url: &str, port: u16) -> bool {
    url::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1")) && u.port() == Some(port)
    })
}

/// Whether a link leaving the app opens in the person's browser: web addresses only. Anything else
/// (files, programs, the system's own handlers such as ms-msdt:) is never handed to the system.
pub fn opens_outside(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}

/// Where a download lands: in `downloads`, under its own file name only (never a path it chose).
pub fn download_path(suggested: &Path, downloads: &Path) -> PathBuf {
    let name = suggested.file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("download");
    downloads.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_shows_the_local_app_only() {
        for ok in
            ["http://localhost:51234/", "http://localhost:51234/dashboards/x?edit", "http://127.0.0.1:51234/explore"]
        {
            assert!(is_app(ok, 51234), "{ok}");
        }
        for elsewhere in [
            "https://giobinda.github.io/magnumbi/docs.html",
            "http://localhost:8000/",
            "http://localhost.evil.example:51234/",
            "http://evil.example/?http://localhost:51234/",
            "file:///etc/passwd",
            "javascript:alert(1)",
        ] {
            assert!(!is_app(elsewhere, 51234), "{elsewhere}");
        }
    }

    #[test]
    fn only_web_addresses_open_in_the_browser() {
        for web in ["https://giobinda.github.io/magnumbi/docs.html", "http://example.com/?a&calc", "HTTPS://x.example/"]
        {
            assert!(opens_outside(web), "{web}");
        }
        // files, programs and the system's own handlers are never handed to the system
        for not in [
            "file:///etc/passwd",
            "file://server/share/x.exe",
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "javascript:alert(1)",
            "smb://server/share",
            "mailto:a@b.example",
            "not a url",
        ] {
            assert!(!opens_outside(not), "{not}");
        }
    }

    #[test]
    fn downloads_land_in_downloads_under_their_own_name() {
        let downloads = Path::new("/home/me/Downloads");
        assert_eq!(download_path(Path::new("sales.csv"), downloads), downloads.join("sales.csv"));
        assert_eq!(download_path(Path::new("/tmp/x/acme-setup.zip"), downloads), downloads.join("acme-setup.zip"));
        assert_eq!(download_path(Path::new("../../.bashrc"), downloads), downloads.join(".bashrc"));
        assert_eq!(download_path(Path::new(""), downloads), downloads.join("download"));
    }
}
