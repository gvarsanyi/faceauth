//! faceauth-settings: GTK 4 / libadwaita front end for faceauth-daemon, for
//! GNOME, COSMIC, and other non-KDE desktops (KDE has the KCM).
//!
//! All daemon calls are blocking D-Bus calls run on GIO's thread pool; the
//! results are applied on the main loop. Changes that need authorization
//! let the desktop's polkit agent prompt.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use faceauth_common::client::Client;
use faceauth_common::config::{ENROLL_CAPTURES, ENROLL_CAPTURE_TIMEOUT_SECS};
use faceauth_common::dbus::{
    preferred_camera, CameraTuple, Error, Result, ENROLLED_CAMERA, STATUS_TOO_SIMILAR, SUITABILITY_IR,
};
use faceauth_common::users;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};

const APP_ID: &str = "io.github.gvarsanyi.faceauth.Settings";
/// Long enough for a whole enrollment plus a polkit prompt.
const METHOD_TIMEOUT: Duration = Duration::from_secs((ENROLL_CAPTURES * ENROLL_CAPTURE_TIMEOUT_SECS + 120) as u64);

struct Ui {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    model_row: adw::ActionRow,
    camera_row: adw::ComboRow,
    add_button: gtk::Button,
    test_button: gtk::Button,
    delete_button: gtk::Button,
    services_group: adw::PreferencesGroup,
    service_rows: RefCell<Vec<adw::SwitchRow>>,
    cameras: RefCell<Vec<CameraTuple>>,
    enrolled: Cell<bool>,
    /// The enrolled camera is connected.
    camera_connected: Cell<bool>,
    client: Option<Arc<Client>>,
    user: String,
}

/// Run `job` against the daemon on a worker thread, then `done` on the main
/// loop. Without a bus connection `done` gets the connection error.
fn run<T, J, D>(ui: &Rc<Ui>, job: J, done: D)
where
    T: Send + 'static,
    J: FnOnce(&Client) -> Result<T> + Send + 'static,
    D: FnOnce(&Rc<Ui>, Result<T>) + 'static,
{
    let ui = ui.clone();
    glib::spawn_future_local(async move {
        let result = match ui.client.clone() {
            Some(client) => gio::spawn_blocking(move || job(&client))
                .await
                .unwrap_or_else(|_| Err(Error::Failed(gettext("internal error (worker thread panicked)")))),
            None => Err(Error::Failed(gettext("cannot connect to the system bus"))),
        };
        done(&ui, result);
    });
}

impl Ui {
    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    /// Show an error unless it is a dismissed polkit prompt; reveal the
    /// banner if the daemon is unreachable.
    fn report(&self, e: &Error) {
        if e.is_unavailable() {
            self.banner.set_revealed(true);
        } else if !matches!(e, Error::NotAuthorized(_) | Error::Cancelled(_)) {
            self.toast(&e.message());
        }
    }

    fn refresh(self: &Rc<Self>) {
        self.banner.set_revealed(false);
        let user = self.user.clone();
        run(self, move |c| c.get_model(&user), Self::show_model);
        run(self, Client::list_cameras, Self::show_cameras);
        let user = self.user.clone();
        run(self, move |c| c.get_services(&user), Self::show_services);
    }

    fn show_model(self: &Rc<Self>, result: Result<faceauth_common::client::ModelInfo>) {
        let subtitle = match &result {
            Ok(m) => {
                let batches = ngettext("{n} enrollment batch", "{n} enrollment batches", m.batches)
                    .replace("{n}", &m.batches.to_string());
                match m.camera_index {
                    Some(i) => format!("{batches}, /dev/video{i}"),
                    None => format!("{batches}; {}", gettext("the enrolled camera is not connected")),
                }
            }
            Err(Error::NoModel(_)) => gettext("No face enrolled"),
            Err(e) => {
                self.report(e);
                String::new()
            }
        };
        self.enrolled.set(result.is_ok());
        self.camera_connected.set(result.as_ref().is_ok_and(|m| m.camera_index.is_some()));
        self.model_row.set_subtitle(&subtitle);
        self.add_button.set_label(&if result.is_ok() { gettext("Add More") } else { gettext("Enroll") });
        // The camera is fixed once enrolled; the model row names it.
        self.camera_row.set_visible(result.is_err());
        self.update_buttons();
    }

    /// Button sensitivity from the model and camera replies, which arrive
    /// in either order.
    fn update_buttons(&self) {
        let enrolled = self.enrolled.get();
        let camera_ready = if enrolled { self.camera_connected.get() } else { !self.cameras.borrow().is_empty() };
        self.add_button.set_sensitive(camera_ready);
        self.test_button.set_sensitive(enrolled && camera_ready);
        self.delete_button.set_sensitive(enrolled);
    }

    fn show_cameras(self: &Rc<Self>, result: Result<Vec<CameraTuple>>) {
        let cameras = result.unwrap_or_else(|e| {
            self.report(&e);
            Vec::new()
        });
        let labels: Vec<String> = cameras
            .iter()
            .map(|(index, name, suitability)| {
                let mut label = format!("/dev/video{index}");
                if !name.is_empty() {
                    label.push_str(&format!(" {name}"));
                }
                if *suitability == SUITABILITY_IR {
                    label.push_str(&format!(" ({})", gettext("IR, recommended")));
                }
                label
            })
            .collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.camera_row.set_model(Some(&gtk::StringList::new(&labels)));
        self.camera_row.set_selected(preferred_camera(&cameras).unwrap_or(0) as u32);
        *self.cameras.borrow_mut() = cameras;
        self.update_buttons();
    }

    fn show_services(self: &Rc<Self>, result: Result<Vec<(String, bool)>>) {
        for row in self.service_rows.borrow_mut().drain(..) {
            self.services_group.remove(&row);
        }
        let services = result.unwrap_or_else(|e| {
            self.report(&e);
            Vec::new()
        });
        self.services_group.set_description(Some(&if services.is_empty() {
            gettext("No services are enabled by the administrator (/etc/faceauth/faceauth.conf).")
        } else {
            gettext("Never used for remote sessions (SSH, remote desktop) or sessions in the background.")
        }));
        for (name, enabled) in services {
            let row = adw::SwitchRow::builder().title(name.as_str()).active(enabled).build();
            if let Some(what) = service_description(&name) {
                row.set_subtitle(&what);
            }
            let ui = self.clone();
            row.connect_active_notify(move |row| {
                let (user, service, on) = (ui.user.clone(), name.clone(), row.is_active());
                run(&ui, move |c| c.set_service_enabled(&user, &service, on), |ui, r| {
                    if let Err(e) = r {
                        ui.report(&e);
                        // Show the stored state again.
                        let user = ui.user.clone();
                        run(ui, move |c| c.get_services(&user), Self::show_services);
                    }
                });
            });
            self.services_group.add(&row);
            self.service_rows.borrow_mut().push(row);
        }
    }

    fn enroll(self: &Rc<Self>) {
        let camera = if self.enrolled.get() {
            ENROLLED_CAMERA
        } else {
            match self.cameras.borrow().get(self.camera_row.selected() as usize) {
                Some(c) => c.0 as i32,
                None => return,
            }
        };

        let progress = gtk::ProgressBar::builder().show_text(true).text(format!("0 / {ENROLL_CAPTURES}")).build();
        let hint = gtk::Label::builder()
            .label(gettext("Look at the camera and turn your head slightly between captures."))
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        let cancel = gtk::Button::with_label(&gettext("Cancel"));
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        content.append(&hint);
        content.append(&progress);
        content.append(&cancel);
        let view = adw::ToolbarView::new();
        view.add_top_bar(&adw::HeaderBar::builder().show_end_title_buttons(false).build());
        view.set_content(Some(&content));
        let dialog = adw::Dialog::builder()
            .title(gettext("Enroll Face"))
            .content_width(380)
            .can_close(false)
            .child(&view)
            .build();
        let ui = self.clone();
        cancel.connect_clicked(move |button| {
            button.set_sensitive(false);
            run(&ui, Client::cancel_enroll, |_, _| {});
        });
        dialog.present(Some(&self.window));

        let (tx, rx) = async_channel::unbounded::<(u32, u32, String)>();
        let bar = progress.clone();
        let label = hint.clone();
        glib::spawn_future_local(async move {
            while let Ok((done, total, status)) = rx.recv().await {
                bar.set_fraction(f64::from(done) / f64::from(total.max(1)));
                bar.set_text(Some(&format!("{done} / {total}")));
                if status == STATUS_TOO_SIMILAR {
                    label.set_label(&gettext("Too similar to a previous capture; turn your head slightly."));
                } else {
                    label.set_label(&gettext("Look at the camera and turn your head slightly between captures."));
                }
            }
        });

        let user = self.user.clone();
        run(
            self,
            move |c| {
                c.enroll(&user, camera, |d, t, s| {
                    let _ = tx.send_blocking((d, t, s.to_string()));
                })
            },
            move |ui, result| {
                dialog.set_can_close(true);
                dialog.close();
                match result {
                    Ok(()) => ui.toast(&gettext("Enrollment complete")),
                    Err(e) => ui.report(&e),
                }
                ui.refresh();
            },
        );
    }

    fn test(self: &Rc<Self>) {
        self.test_button.set_sensitive(false);
        self.toast(&gettext("Look at the camera..."));
        let user = self.user.clone();
        run(self, move |c| c.authenticate(&user, "", 0), |ui, result| {
            ui.test_button.set_sensitive(true);
            match result {
                Ok(()) => ui.toast(&gettext("Face recognized")),
                Err(e) if e.is_unavailable() => ui.report(&e),
                Err(e) => ui.toast(&gettext("Not recognized: {error}").replace("{error}", &e.message())),
            }
        });
    }

    fn delete(self: &Rc<Self>) {
        let dialog = adw::AlertDialog::new(
            Some(&gettext("Delete Face Data?")),
            Some(&gettext("All enrolled face data is removed. You need to enroll again to use face authentication.")),
        );
        dialog.add_responses(&[("cancel", &gettext("Cancel")), ("delete", &gettext("Delete"))]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        let ui = self.clone();
        dialog.connect_response(Some("delete"), move |_, _| {
            let user = ui.user.clone();
            run(&ui, move |c| c.delete_model(&user), |ui, r| {
                if let Err(e) = r {
                    ui.report(&e);
                }
                ui.refresh();
            });
        });
        dialog.present(Some(&self.window));
    }
}

/// What a well-known PAM service is used for.
fn service_description(service: &str) -> Option<String> {
    Some(match service {
        "sudo" => gettext("Administrator commands in a terminal"),
        "sudo-i" => gettext("Administrator login shells (sudo -i)"),
        "polkit-1" => gettext("Authorization prompts of applications"),
        "kde-fingerprint" => gettext("KDE Plasma lock screen"),
        "gdm-password" | "sddm" | "lightdm" | "cosmic-greeter" => gettext("Screen unlock (not used for login)"),
        "login" => gettext("Text console login"),
        "su" => gettext("Switching user (su)"),
        _ => return None,
    })
}

fn build_ui(app: &adw::Application) {
    let user = users::by_uid(users::current_uid()).map(|u| u.name).unwrap_or_default();
    let client = Client::from_env(METHOD_TIMEOUT).ok().map(Arc::new);

    let banner = adw::Banner::builder()
        .title(gettext("The face authentication service is not available"))
        .button_label(gettext("Retry"))
        .build();

    let model_row = adw::ActionRow::builder().title(gettext("Face data")).build();
    let camera_row = adw::ComboRow::builder().title(gettext("Camera")).build();
    let add_button = gtk::Button::builder().label(gettext("Enroll")).css_classes(["suggested-action"]).build();
    let test_button = gtk::Button::with_label(&gettext("Test"));
    let delete_button = gtk::Button::builder().label(gettext("Delete")).css_classes(["destructive-action"]).build();
    let buttons = gtk::Box::builder().spacing(12).halign(gtk::Align::End).margin_top(12).build();
    buttons.append(&delete_button);
    buttons.append(&test_button);
    buttons.append(&add_button);

    let face_group = adw::PreferencesGroup::builder().title(gettext("Face")).build();
    face_group.add(&model_row);
    face_group.add(&camera_row);
    face_group.add(&buttons);
    let services_group = adw::PreferencesGroup::builder().title(gettext("Use Face Authentication For")).build();

    let page = adw::PreferencesPage::new();
    page.add(&face_group);
    page.add(&services_group);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&page));

    let refresh = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text(gettext("Reload")).build();
    let header = adw::HeaderBar::new();
    header.pack_start(&refresh);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.add_top_bar(&banner);
    view.set_content(Some(&toasts));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(gettext("Face Authentication"))
        .default_width(560)
        .default_height(640)
        .content(&view)
        .build();

    let ui = Rc::new(Ui {
        window,
        toasts,
        banner,
        model_row,
        camera_row,
        add_button,
        test_button,
        delete_button,
        services_group,
        service_rows: RefCell::default(),
        cameras: RefCell::default(),
        enrolled: Cell::new(false),
        camera_connected: Cell::new(false),
        client,
        user,
    });
    let u = ui.clone();
    ui.banner.connect_button_clicked(move |_| u.refresh());
    let u = ui.clone();
    refresh.connect_clicked(move |_| u.refresh());
    let u = ui.clone();
    ui.add_button.connect_clicked(move |_| u.enroll());
    let u = ui.clone();
    ui.test_button.connect_clicked(move |_| u.test());
    let u = ui.clone();
    ui.delete_button.connect_clicked(move |_| u.delete());

    ui.refresh();
    ui.window.present();
}

fn main() -> glib::ExitCode {
    gettextrs::setlocale(gettextrs::LocaleCategory::LcAll, "");
    let _ = gettextrs::bindtextdomain("faceauth", faceauth_common::LOCALE_DIR);
    let _ = gettextrs::textdomain("faceauth");

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}
