#[cfg(all(target_os = "linux", feature = "tray"))]
pub(crate) mod tray_support {
    use ksni::blocking::TrayMethods;
    use ksni::menu::StandardItem;
    use std::sync::mpsc::{self, Receiver, Sender};

    pub enum Command {
        Show,
        Hide,
        Quit,
    }

    struct TrayMenu {
        sender: Sender<Command>,
        show_label: String,
        hide_label: String,
        quit_label: String,
    }

    impl ksni::Tray for TrayMenu {
        fn id(&self) -> String {
            "qpwgraph-rs".into()
        }

        fn title(&self) -> String {
            "qpwgraph-rs".into()
        }

        fn icon_name(&self) -> String {
            "audio-card".into()
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.sender.send(Command::Show);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            let show_sender = self.sender.clone();
            let hide_sender = self.sender.clone();
            let quit_sender = self.sender.clone();
            vec![
                StandardItem {
                    label: self.show_label.clone(),
                    activate: Box::new(move |_| {
                        let _ = show_sender.send(Command::Show);
                    }),
                    ..Default::default()
                }
                .into(),
                StandardItem {
                    label: self.hide_label.clone(),
                    activate: Box::new(move |_| {
                        let _ = hide_sender.send(Command::Hide);
                    }),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                StandardItem {
                    label: self.quit_label.clone(),
                    activate: Box::new(move |_| {
                        let _ = quit_sender.send(Command::Quit);
                    }),
                    ..Default::default()
                }
                .into(),
            ]
        }
    }

    pub struct State {
        pub receiver: Receiver<Command>,
        handle: ksni::blocking::Handle<TrayMenu>,
    }

    pub fn start(show_label: String, hide_label: String, quit_label: String) -> Option<State> {
        let (sender, receiver) = mpsc::channel();
        let tray = TrayMenu {
            sender,
            show_label,
            hide_label,
            quit_label,
        };
        let handle = tray.spawn().ok()?;
        Some(State { receiver, handle })
    }

    impl State {
        pub fn shutdown(&self) {
            self.handle.shutdown().wait();
        }
    }
}
