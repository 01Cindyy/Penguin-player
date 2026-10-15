//! The Plugins page: the plugins shipped with the app and the user's own, each with a
//! switch, its last error and its settings; where lyrics that plugins find are kept;
//! and where to learn to write plugins.
//!
//! Switching on a plugin that wants to go online or use Discord first asks the user,
//! naming the sites it may reach.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use mp_core::PluginPermissions;
use mp_plugins::{PluginInfo, SettingKind, SettingSpec};
use serde_json::Value;

use crate::context::AppContext;
use crate::ui::icons;
use crate::ui::{disconnect_on_destroy, keep_alive, set_tooltip};

pub(super) fn page(ctx: &Rc<AppContext>) -> adw::PreferencesPage {
    let plugins = Rc::new(Plugins::new(ctx));
    plugins.rebuild();
    plugins.connect_signals();

    let page = adw::PreferencesPage::builder()
        .name("plugins")
        .title("Plugins")
        .icon_name(icons::PLUGINS)
        .build();
    page.add(&plugins.included);
    page.add(&plugins.installed);
    page.add(&lyrics_group(ctx));
    page.add(&guide_group());
    keep_alive(&page, plugins);
    page
}

/// A plugin's row and the parts that follow the plugin service.
struct PluginRow {
    id: String,
    bundled: bool,
    row: adw::ActionRow,
    switch: gtk::Switch,
    error_button: gtk::MenuButton,
    error_label: gtk::Label,
}

/// The page's state, owned by the page (see [`keep_alive`]).
struct Plugins {
    ctx: Rc<AppContext>,
    /// The plugins shipped with the app.
    included: adw::PreferencesGroup,
    /// The user's own plugins.
    installed: adw::PreferencesGroup,
    open_folder: gtk::Button,
    reload: gtk::Button,
    rows: RefCell<Vec<PluginRow>>,
    /// Shown instead of rows while the user has no plugins of their own.
    empty_row: adw::ActionRow,
}

impl Plugins {
    fn new(ctx: &Rc<AppContext>) -> Self {
        let open_folder = super::header_button(icons::OPEN_FOLDER, "Open Plugins Folder");
        let reload = super::header_button(icons::REFRESH, "Reload Plugins");
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.append(&open_folder);
        actions.append(&reload);
        let included = adw::PreferencesGroup::builder()
            .title("Included Plugins")
            .description(format!(
                "Made for {}. Like any plugin, they stay off until you switch them on.",
                mp_core::APP_NAME
            ))
            .build();
        let installed = adw::PreferencesGroup::builder()
            .title("Your Plugins")
            .description(
                "Plugins are Lua scripts that run in a sandbox. Reload them after \
                 changing their files.",
            )
            .header_suffix(&actions)
            .build();
        let empty_row = adw::ActionRow::builder()
            .title("No Plugins Yet")
            .subtitle("Put a plugin’s folder in the plugins folder, then reload.")
            .css_classes(["mp-plugins-empty"])
            .build();
        Self {
            ctx: ctx.clone(),
            included,
            installed,
            open_folder,
            reload,
            rows: RefCell::default(),
            empty_row,
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.open_folder.connect_clicked(move |button| {
            if let Some(plugins) = weak.upgrade() {
                super::open_folder(plugins.ctx.plugins.plugins_dir(), button);
            }
        });
        let weak = Rc::downgrade(self);
        self.reload.connect_clicked(move |button| {
            if let Some(plugins) = weak.upgrade() {
                plugins.ctx.plugins.reload();
                plugins.rebuild();
                super::toast(button, "Plugins reloaded");
            }
        });
        let weak = Rc::downgrade(self);
        let handler = self.ctx.plugins.connect_changed(move |_| {
            if let Some(plugins) = weak.upgrade() {
                plugins.update_states();
            }
        });
        disconnect_on_destroy(&self.installed, &self.ctx.plugins, handler);
    }

    /// Lists the plugins again.
    fn rebuild(&self) {
        for old in self.rows.take() {
            self.group(old.bundled).remove(&old.row);
        }
        if self.empty_row.parent().is_some() {
            self.installed.remove(&self.empty_row);
        }
        let rows: Vec<PluginRow> = self
            .ctx
            .plugins
            .discover()
            .iter()
            .map(|info| plugin_row(&self.ctx, info))
            .collect();
        if !rows.iter().any(|row| !row.bundled) {
            self.installed.add(&self.empty_row);
        }
        self.included
            .set_visible(rows.iter().any(|row| row.bundled));
        for row in &rows {
            self.group(row.bundled).add(&row.row);
        }
        self.rows.replace(rows);
        self.update_states();
    }

    fn group(&self, bundled: bool) -> &adw::PreferencesGroup {
        if bundled {
            &self.included
        } else {
            &self.installed
        }
    }

    /// Brings switches and errors up to date with the plugin service.
    fn update_states(&self) {
        let service = &self.ctx.plugins;
        for plugin in self.rows.borrow().iter() {
            if plugin.switch.is_sensitive() {
                let enabled = service.is_enabled(&plugin.id);
                plugin.switch.set_active(enabled);
                plugin.switch.set_state(enabled);
            }
            let error = service.last_error(&plugin.id);
            plugin
                .error_label
                .set_label(error.as_deref().unwrap_or_default());
            plugin.error_button.set_visible(error.is_some());
        }
    }
}

fn plugin_row(ctx: &Rc<AppContext>, info: &PluginInfo) -> PluginRow {
    let manifest = &info.manifest;
    let row = adw::ActionRow::builder()
        .title(&manifest.name)
        .use_markup(false)
        .build();
    let switch = gtk::Switch::builder()
        .valign(gtk::Align::Center)
        .sensitive(info.problem.is_none())
        .build();
    switch.update_property(&[gtk::accessible::Property::Label(&format!(
        "Use {}",
        manifest.name
    ))]);
    match &info.problem {
        Some(problem) => {
            row.set_subtitle(&format!("Can’t be loaded: {problem}"));
            row.add_css_class("mp-plugin-broken");
            row.add_prefix(&super::alert_icon("error"));
        }
        None => row.set_subtitle(&subtitle(info)),
    }

    let (error_button, error_label) = error_button();
    row.add_suffix(&error_button);
    if info.problem.is_none() && !manifest.settings.is_empty() {
        row.add_suffix(&settings_button(ctx, info));
    }
    row.add_suffix(&switch);
    row.set_activatable_widget(Some(&switch));

    let bundled = info.is_bundled();
    let id = info.id.clone();
    let info = info.clone();
    switch.connect_state_set(glib::clone!(
        #[weak]
        ctx,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |switch, on| {
            let wanted = &info.manifest.permissions;
            if on && !ctx.plugins.permissions(&info.id).covers(wanted) {
                ask_to_allow(&ctx, &info, switch);
                // The switch stays half on until the user answers.
                return glib::Propagation::Stop;
            }
            ctx.plugins.set_enabled(&info.id, on);
            glib::Propagation::Proceed
        }
    ));
    PluginRow {
        id,
        bundled,
        row,
        switch,
        error_button,
        error_label,
    }
}

/// The description, what the plugin reaches beyond the app, and who made which version.
fn subtitle(info: &PluginInfo) -> String {
    let manifest = &info.manifest;
    let author = (!info.is_bundled() && !manifest.author.is_empty())
        .then(|| format!("by {}", manifest.author));
    let version = (!manifest.version.is_empty()).then(|| format!("Version {}", manifest.version));
    let byline: Vec<String> = [version, author].into_iter().flatten().collect();
    let access = access_summary(&manifest.permissions);
    [manifest.description.clone(), access, byline.join(" · ")]
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// One line naming what a plugin may reach, such as "Connects to lrclib.net".
fn access_summary(permissions: &PluginPermissions) -> String {
    let mut parts = Vec::new();
    if !permissions.network.is_empty() {
        parts.push(format!("Connects to {}", list(&permissions.network)));
    }
    if permissions.discord {
        parts.push("Uses Discord".to_owned());
    }
    parts.join(" · ")
}

/// "a", "a and b", "a, b and c".
fn list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Asks whether the plugin may do what it asks for, then switches it on or back off.
fn ask_to_allow(ctx: &Rc<AppContext>, info: &PluginInfo, switch: &gtk::Switch) {
    let manifest = &info.manifest;
    let permissions = &manifest.permissions;
    let mut paragraphs = Vec::new();
    if !permissions.network.is_empty() {
        paragraphs.push(format!(
            "It will connect to {}, and can send information about your music there.",
            list(&permissions.network)
        ));
    }
    if permissions.discord {
        paragraphs.push(
            "It will show what you’re listening to on your Discord profile while Discord \
             is open."
                .to_owned(),
        );
    }
    if !manifest.privacy.trim().is_empty() {
        paragraphs.push(manifest.privacy.trim().to_owned());
    }
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Allow “{}”?", manifest.name))
        .body(paragraphs.join("\n\n"))
        .default_response("allow")
        .close_response("cancel")
        .build();
    dialog.add_responses(&[("cancel", "_Cancel"), ("allow", "_Allow")]);
    dialog.set_response_appearance("allow", adw::ResponseAppearance::Suggested);
    let id = info.id.clone();
    let permissions = permissions.clone();
    dialog.connect_response(
        None,
        glib::clone!(
            #[weak]
            ctx,
            #[weak]
            switch,
            move |_, response| {
                if response == "allow" {
                    ctx.plugins.allow(&id, &permissions);
                    ctx.plugins.set_enabled(&id, true);
                    switch.set_state(true);
                } else {
                    switch.set_active(false);
                }
            }
        ),
    );
    dialog.present(Some(switch));
}

/// Opens the plugin's settings over the plugins page.
fn settings_button(ctx: &Rc<AppContext>, info: &PluginInfo) -> gtk::Button {
    let button = crate::ui::icon_button(
        icons::PREFERENCES,
        &format!("{} Settings", info.manifest.name),
        &["flat"],
    );
    let info = info.clone();
    button.connect_clicked(glib::clone!(
        #[weak]
        ctx,
        move |button| {
            let dialog = button
                .ancestor(adw::PreferencesDialog::static_type())
                .and_downcast::<adw::PreferencesDialog>();
            if let Some(dialog) = dialog {
                dialog.push_subpage(&settings_page(&ctx, &info));
            }
        }
    ));
    button
}

fn settings_page(ctx: &Rc<AppContext>, info: &PluginInfo) -> adw::NavigationPage {
    let manifest = &info.manifest;
    let settings = adw::PreferencesGroup::builder()
        .description(&manifest.description)
        .build();
    for spec in &manifest.settings {
        settings.add(&setting_row(ctx, &info.id, spec));
    }
    let page = adw::PreferencesPage::new();
    page.add(&settings);
    if !manifest.permissions.is_empty() {
        page.add(&access_group(info));
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&page));
    adw::NavigationPage::builder()
        .title(&manifest.name)
        .child(&toolbar)
        .build()
}

/// What the plugin may reach beyond the app, and what it says it sends there.
fn access_group(info: &PluginInfo) -> adw::PreferencesGroup {
    let manifest = &info.manifest;
    let group = adw::PreferencesGroup::builder()
        .title("Access")
        .description(&manifest.privacy)
        .build();
    for host in &manifest.permissions.network {
        group.add(
            &adw::ActionRow::builder()
                .title(host)
                .subtitle("Can receive information about your music")
                .use_markup(false)
                .build(),
        );
    }
    if manifest.permissions.discord {
        group.add(
            &adw::ActionRow::builder()
                .title("Discord")
                .subtitle("Can show what you’re listening to on your profile")
                .build(),
        );
    }
    group
}

/// A row for one of a plugin's settings, saving every change. Values equal to the
/// default aren't saved, so they follow the plugin if its default changes.
fn setting_row(ctx: &Rc<AppContext>, id: &str, spec: &SettingSpec) -> gtk::Widget {
    let current = spec.value_or_default(ctx.plugins.setting(id, &spec.key).as_ref());
    let save = {
        let ctx = Rc::downgrade(ctx);
        let id = id.to_owned();
        let spec = spec.clone();
        move |value: Value| {
            let value = spec.value_or_default(Some(&value));
            let changed = (value != spec.default_value()).then_some(value);
            if let Some(ctx) = ctx.upgrade() {
                ctx.plugins.set_setting(&id, &spec.key, changed);
            }
        }
    };
    let subtitle = Some(spec.subtitle.as_str()).filter(|subtitle| !subtitle.is_empty());
    match &spec.kind {
        SettingKind::Boolean { .. } => {
            let row = adw::SwitchRow::builder()
                .title(&spec.title)
                .active(current.as_bool().unwrap_or_default())
                .build();
            row.set_subtitle(subtitle.unwrap_or_default());
            row.connect_active_notify(move |row| save(Value::Bool(row.is_active())));
            row.upcast()
        }
        SettingKind::Number { min, max, step, .. } => {
            let row = adw::SpinRow::with_range(*min, *max, *step);
            row.set_title(&spec.title);
            row.set_subtitle(subtitle.unwrap_or_default());
            row.set_digits(decimals(*step));
            row.set_value(current.as_f64().unwrap_or(*min));
            row.connect_value_notify(move |row| save(Value::from(row.value())));
            row.upcast()
        }
        SettingKind::Choice { choices, .. } => {
            let titles: Vec<&str> = choices.iter().map(|choice| choice.title.as_str()).collect();
            let row = adw::ComboRow::builder()
                .title(&spec.title)
                .model(&gtk::StringList::new(&titles))
                .build();
            row.set_subtitle(subtitle.unwrap_or_default());
            let selected = choices
                .iter()
                .position(|choice| current.as_str() == Some(choice.value.as_str()));
            row.set_selected(
                selected
                    .and_then(|index| u32::try_from(index).ok())
                    .unwrap_or(0),
            );
            let values: Vec<String> = choices.iter().map(|choice| choice.value.clone()).collect();
            row.connect_selected_notify(move |row| {
                if let Some(value) = values.get(row.selected() as usize) {
                    save(Value::String(value.clone()));
                }
            });
            row.upcast()
        }
    }
}

/// Decimal places to show for numbers in steps of `step`.
fn decimals(step: f64) -> u32 {
    (0..3)
        .find(|places| (step * 10f64.powi(*places as i32)).fract().abs() < 1e-9)
        .unwrap_or(3)
}

/// Where lyrics that plugins find are kept.
fn lyrics_group(ctx: &Rc<AppContext>) -> adw::PreferencesGroup {
    let save = adw::SwitchRow::builder()
        .title("Save Next to Songs")
        .subtitle(
            "Keep lyrics that plugins find as .lrc files beside the songs, where other \
             players can use them too. Otherwise, or where a song has one already, \
             they’re kept in the app’s cache.",
        )
        .active(ctx.settings().save_found_lyrics)
        .build();
    save.connect_active_notify(glib::clone!(
        #[weak]
        ctx,
        move |row| ctx.update_settings(|settings| settings.save_found_lyrics = row.is_active())
    ));
    let group = adw::PreferencesGroup::builder()
        .title("Lyrics From Plugins")
        .build();
    group.add(&save);
    group
}

/// A small "Error" button whose popover shows a plugin's last error in full.
fn error_button() -> (gtk::MenuButton, gtk::Label) {
    let label = gtk::Label::builder()
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .max_width_chars(48)
        .xalign(0.0)
        .css_classes(["mp-plugin-error", "monospace"])
        .build();
    let heading = gtk::Label::builder()
        .label("Last Error")
        .xalign(0.0)
        .css_classes(["heading"])
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    content.append(&heading);
    content.append(&label);
    let button = gtk::MenuButton::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name(icons::WARNING)
                .label("Error")
                .build(),
        )
        .valign(gtk::Align::Center)
        .visible(false)
        .popover(&gtk::Popover::builder().child(&content).build())
        .css_classes(["flat", "mp-plugin-error-button"])
        .build();
    set_tooltip(&button, "Show the Last Error");
    (button, label)
}

/// Where plugin authors start.
fn guide_group() -> adw::PreferencesGroup {
    let guide = adw::ActionRow::builder()
        .title("Writing Plugins")
        .subtitle(
            "The Lua API is described in docs/plugins.md, and examples/plugins holds \
             plugins to start from.",
        )
        .build();
    guide.add_prefix(&gtk::Image::from_icon_name(icons::GUIDE));
    let group = adw::PreferencesGroup::new();
    group.add(&guide);
    group
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_decimals() {
        let hosts = |hosts: &[&str]| hosts.iter().map(|h| h.to_string()).collect::<Vec<_>>();
        assert_eq!(list(&hosts(&["a"])), "a");
        assert_eq!(list(&hosts(&["a", "b"])), "a and b");
        assert_eq!(list(&hosts(&["a", "b", "c"])), "a, b and c");
        assert_eq!(decimals(1.0), 0);
        assert_eq!(decimals(0.5), 1);
        assert_eq!(decimals(0.25), 2);
    }
}
