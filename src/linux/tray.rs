//! Tray icon (StatusNotifierItem). KDE, XFCE, Cinnamon and most panels show it
//! out of the box; GNOME needs the AppIndicator extension, which Debian ships as
//! gnome-shell-extension-appindicator.
use super::{post, UserEvent};
use ksni::blocking::TrayMethods;
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip, Tray};

struct ApertureTray {
    icons: Vec<Icon>,
    /// (label, action) pairs; a separator goes before the record entries
    labels: Vec<(String, usize)>,
}

fn item(label: &str, ev: impl Fn() -> UserEvent + Send + 'static) -> MenuItem<ApertureTray> {
    StandardItem { label: label.into(), activate: Box::new(move |_| post(ev())), ..Default::default() }.into()
}

impl Tray for ApertureTray {
    fn id(&self) -> String {
        "aperture".into()
    }
    fn title(&self) -> String {
        "Aperture".into()
    }
    fn icon_name(&self) -> String {
        "aperture".into()
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        self.icons.clone()
    }
    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "Aperture".into(), description: "Click for settings · right-click to capture".into(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        post(UserEvent::Settings);
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut m: Vec<MenuItem<Self>> = Vec::new();
        for (l, act) in self.labels.iter() {
            let act = *act;
            if act == 5 {
                m.push(MenuItem::Separator);
            }
            // a short delay lets the menu disappear before the screen is grabbed
            m.push(item(l, move || UserEvent::Delayed(act)));
        }
        m.push(MenuItem::Separator);
        m.push(item("Settings & history", || UserEvent::Settings));
        m.push(MenuItem::Separator);
        m.push(item("Quit Aperture", || UserEvent::Quit));
        m
    }
}

fn icon(size: i32) -> Icon {
    let img = crate::icons::render_camera(size);
    let mut data = Vec::with_capacity(img.px.len() * 4);
    for p in &img.px {
        data.extend_from_slice(&p.to_be_bytes()); // ARGB32, network byte order
    }
    Icon { width: size, height: size, data }
}

pub struct Handle(Option<ksni::blocking::Handle<ApertureTray>>);

pub fn start(labels: Vec<(String, usize)>) -> Handle {
    let t = ApertureTray { icons: vec![icon(22), icon(32), icon(48)], labels };
    Handle(t.spawn().ok())
}

impl Handle {
    pub fn set_labels(&self, labels: Vec<(String, usize)>) {
        if let Some(h) = &self.0 {
            h.update(move |t| t.labels = labels);
        }
    }
    pub fn available(&self) -> bool {
        self.0.is_some()
    }
}
