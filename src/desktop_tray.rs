use std::{io::Cursor, sync::mpsc::Receiver};

use anyhow::{Context, Result};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuId, MenuItem},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    window::WindowId,
};

#[cfg(target_os = "macos")]
use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

#[derive(Debug)]
enum UserEvent {
    Menu(MenuEvent),
    ServerReady,
}

pub(crate) fn run(management_url: String, server_ready: Receiver<()>) -> Result<()> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false)
            .with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder.build().context("create desktop tray event loop")?;
    let menu_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        let _ = menu_proxy.send_event(UserEvent::Menu(event));
    }));
    let ready_proxy = event_loop.create_proxy();
    std::thread::Builder::new()
        .name("chatcmd-browser-open".to_owned())
        .spawn(move || {
            if server_ready.recv().is_ok() {
                let _ = ready_proxy.send_event(UserEvent::ServerReady);
            }
        })
        .context("start browser-open watcher")?;

    let open_item = MenuItem::new("Vào trang quản lý", true, None);
    let quit_item = MenuItem::new("Thoát", true, None);
    let mut app = TrayApplication {
        tray: None,
        management_url,
        open_id: open_item.id().clone(),
        quit_id: quit_item.id().clone(),
        open_item,
        quit_item,
        browser_opened: false,
    };
    event_loop
        .run_app(&mut app)
        .context("run desktop tray event loop")
}

struct TrayApplication {
    tray: Option<TrayIcon>,
    management_url: String,
    open_id: MenuId,
    quit_id: MenuId,
    open_item: MenuItem,
    quit_item: MenuItem,
    browser_opened: bool,
}

impl ApplicationHandler<UserEvent> for TrayApplication {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.tray.is_none()
            && let Ok(tray) = build_tray(&self.open_item, &self.quit_item)
        {
            self.tray = Some(tray);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ServerReady if !self.browser_opened => {
                let _ = webbrowser::open(&self.management_url);
                self.browser_opened = true;
            }
            UserEvent::ServerReady => {}
            UserEvent::Menu(event) if event.id == self.open_id => {
                let _ = webbrowser::open(&self.management_url);
            }
            UserEvent::Menu(event) if event.id == self.quit_id => event_loop.exit(),
            UserEvent::Menu(_) => {}
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }
}

fn build_tray(open_item: &MenuItem, quit_item: &MenuItem) -> Result<TrayIcon> {
    let menu = Menu::new();
    menu.append_items(&[open_item, quit_item])
        .context("create ChatCMD tray menu")?;
    let icon = load_icon()?;
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("ChatCMD")
        .with_icon(icon)
        .with_menu_on_left_click(true)
        .with_menu_on_right_click(true);
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_as_template(true);
    builder.build().context("create ChatCMD tray icon")
}

fn load_icon() -> Result<Icon> {
    let decoder = png::Decoder::new(Cursor::new(include_bytes!(
        "../assets/icons/favicon-32x32.png"
    )));
    let mut reader = decoder.read_info().context("decode tray icon header")?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .context("decode tray icon pixels")?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buffer[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buffer[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
            .collect(),
        _ => anyhow::bail!("unsupported tray icon color type"),
    };
    Icon::from_rgba(rgba, info.width, info.height).context("create tray icon")
}
