#![cfg(test)]

use super::*;
use crate::core::fetch::{Chunk, Fetched, Outcome};
use crate::core::settings::Settings;
use crate::core::text::tests::english;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_HOME};

#[test]
fn show_in_commander_gives_the_destination_panel_keyboard_focus() {
    win::shell::initialize_com();
    win::shell::initialize_ole();
    win::controls::initialize();
    let class = "gozler.test.show-in-commander-focus";
    win::window::register_class::<MainWindow>(
        class,
        crate::core::ids::IconId::Gozler.id(),
        win::window::Background::Window,
    )
    .expect("the test's window class registers");
    win::pane::register().expect("the pane class registers once in this test process");

    let mut model = Model::new(english(), Settings::default(), Instance::Another);
    model.set_typed_search(List::Search, "navigation-focus-fixture".into());
    let fixture =
        std::env::temp_dir().join(format!("gozler-navigation-focus-{}", std::process::id()));
    std::fs::create_dir_all(fixture.join("child")).expect("the fixture directory is writable");
    let selected = fixture.join("selected.txt");
    std::fs::write(&selected, b"fixture").expect("the fixture file is writable");
    let settings_file = std::env::temp_dir().join(format!(
        "gozler-navigation-focus-{}.json",
        std::process::id()
    ));
    let (hwnd, window) = MainWindow::open(class, model, settings_file, 0, false);
    struct Opened(Hwnd, Rc<MainWindow>, PathBuf);
    impl Drop for Opened {
        fn drop(&mut self) {
            self.0.destroy();
            self.1.finish_shutdown();
            std::fs::remove_file(self.2.join("selected.txt")).expect("the fixture file is ours");
            std::fs::remove_dir(self.2.join("child")).expect("the child fixture is empty");
            std::fs::remove_dir(&self.2).expect("the root fixture is empty");
        }
    }
    let _opened = Opened(hwnd, Rc::clone(&window), fixture.clone());
    let generation = window
        .created()
        .work
        .borrow()
        .requests
        .current(Purpose::Results(List::Search))
        .expect("opening the window starts its search");
    window
        .created()
        .work
        .borrow_mut()
        .requests
        .cancel(Purpose::Results(List::Search));
    let accepted = window.model.borrow_mut().accept(Fetched {
        purpose: Purpose::Results(List::Search),
        generation,
        outcome: Outcome::Rows(Chunk {
            start: 0,
            rows: vec![ResultItem {
                path: selected.to_string_lossy().into_owned(),
                path_u16: None,
                size: None,
                mtime_ft: None,
                created_ft: None,
                is_dir: false,
                attributes: None,
            }]
            .into(),
            total: 1,
            volumes_incomplete: false,
            metadata_pending: false,
            sums: None,
            rest_follows: false,
            made: None,
        }),
    });
    window.apply(hwnd, accepted);
    let search = window.controls().search.row.input.hwnd();
    search.focus();
    assert_eq!(Hwnd::focused(), Some(search));
    hwnd.set_timer(0x7fff, 10);
    let until = |done: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(3);
        win::window::run_messages_until(hwnd, &|| window.accelerators(), &|| {
            done() || Instant::now() >= deadline
        });
        assert!(
            done(),
            "the native navigation state must finish within the test budget"
        );
    };
    window.controls().search.list.select_only(Some(0));
    window.run(hwnd, MenuItem::Run(CommandId::ShowInCommander));
    until(&|| window.model.borrow().mode() == Mode::Commander);
    assert!(window.model.borrow().mode() == Mode::Commander);
    let side = window.model.borrow().panels().active();
    let list = window.panel(side).list;
    until(&|| list.count() == 3 && list.focused().is_some());
    win::window::post_key(list.hwnd(), VK_HOME);
    assert_eq!(
        Hwnd::focused(),
        Some(window.home()),
        "Show in Commander must give the destination panel its keyboard focus"
    );
    until(&|| list.focused() == Some(0));
    win::window::post_key(list.hwnd(), VK_DOWN);
    until(&|| list.focused() == Some(1));
    hwnd.kill_timer(0x7fff);
}
