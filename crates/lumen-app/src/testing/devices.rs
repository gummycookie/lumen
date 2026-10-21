//! What the end-to-end tests may see and do with devices and plugins
//! (the chain, the browser, hosts, presets, parameters, editors).

use super::*;

impl App {
    /// Gives a plugin's parameter edits as if its own window made them:
    /// `(parameter, value, "begin" | "perform" | "end")` each, in order.
    pub fn pretend_plugin_edits(&mut self, device: Id, edits: &[(u32, f64, &str)]) {
        use lumen_hosting::{Edit, EditKind};
        let edits = edits
            .iter()
            .map(|&(id, value, kind)| {
                let kind = match kind {
                    "begin" => EditKind::Begin,
                    "perform" => EditKind::Perform,
                    "end" => EditKind::End,
                    other => panic!("an edit: begin, perform or end, not {other}"),
                };
                (Edit { id, value, kind }, true)
            })
            .collect();
        self.hosting.pretend_edits(device, edits);
    }

    /// The name of the sound the card of `device` shows (the field), and
    /// whether it is marked changed; none when the card has no field or no
    /// sound was chosen.
    pub fn device_preset(&self, device: Id) -> Option<(String, bool)> {
        let look = self.field_look(device)?;
        look.name.map(|name| (name, look.changed))
    }

    /// Whether the card of `device` shows a preset field.
    pub fn device_has_preset_field(&self, device: Id) -> bool {
        self.field_look(device).is_some()
    }

    /// Gives a plugin effect a sidechain input, and the light of `track`'s
    /// sidechain a level (0 to 1) and, when `silent_seconds` is over two
    /// while the song plays, the warning ring: screenshots of the card's row.
    pub fn give_device_sidechain(
        &mut self,
        device: Id,
        track: Id,
        level: f32,
        silent_seconds: f32,
    ) {
        self.hosting.pretend_sidechain_input(device);
        self.chain.sidechain.pretend(track, level, silent_seconds);
    }

    /// Gives a plugin instrument `extra` outputs besides its main one, as if
    /// its host had listed them: screenshots of the card's outputs row.
    pub fn give_device_outputs(&mut self, device: Id, extra: &[&str]) {
        self.hosting.pretend_outputs(device, extra);
    }

    /// How lit the sidechain light of the shown track is, 0 to 1.
    pub fn sidechain_light(&self, track: Id) -> f32 {
        self.chain.sidechain.level_of(track)
    }

    /// The folders "Show presets folder" opened, oldest first.
    pub fn revealed_folders(&self) -> Vec<PathBuf> {
        self.chain.presets.revealed.clone()
    }

    /// Whether Lumen is in safe mode, and whether it is offering it.
    pub fn safe_mode(&self) -> (bool, bool) {
        (self.safe.on, self.safe.offer)
    }

    /// How many plugin hosts exist.
    pub fn plugin_hosts(&self) -> usize {
        self.hosting.host_count()
    }

    /// Loads one plugin in safe mode, as its card's "Load this plugin" will.
    pub fn load_one(&mut self, device: Id) {
        self.load_one_plugin(device);
    }

    /// How a plugin device's host is doing: `loading`, `ready`, `failed: ...`
    /// or `missing`; none when the project holds no such device.
    pub fn device_state(&self, device: Id) -> Option<String> {
        use crate::hosting::State;
        self.hosting.state(device).map(|s| match s {
            State::Loading => "loading".to_owned(),
            State::Ready => "ready".to_owned(),
            State::Failed(why) => format!("failed: {why}"),
            State::Missing => "missing".to_owned(),
            State::Safe => "safe".to_owned(),
        })
    }

    /// The longest block a plugin device was promised (the device's block,
    /// at most the engine's), once its host has linked.
    pub fn device_promised_frames(&self, device: Id) -> Option<u32> {
        self.hosting.promised(device)
    }

    /// Shows the audio panel's block budget with these plugins, whatever
    /// runs, for screenshots: a block of `block_us` that used `used_us`, and
    /// each plugin's name, place, work and wait (microseconds a block) and
    /// missed blocks. Held for an hour.
    pub fn pretend_budget(
        &mut self,
        block_us: u64,
        used_us: u64,
        plugins: &[(&str, &str, u64, u64, u32)],
    ) {
        use crate::transport::budget::{Share, Snapshot};
        let us = Duration::from_micros;
        let plugins: Vec<Share> = plugins
            .iter()
            .enumerate()
            .map(|(i, &(name, place, work, waited, missed))| Share {
                device: Id(i as u64 + 1),
                name: name.into(),
                place: place.into(),
                work: us(work),
                waited: us(waited),
                missed,
            })
            .collect();
        let waited: Duration = plugins.iter().map(|p| p.waited).sum();
        let (part, whole) = lumen_engine::HOST_SHARE;
        self.transport.budget.pretend(Snapshot {
            block: us(block_us),
            used: us(used_us),
            deadline: us(block_us * part / whole),
            lumen: us(used_us).saturating_sub(waited),
            plugins,
        });
    }

    /// Shows a plugin device as `loading`, `ready`, `failed` or `missing`
    /// whatever its host is doing (screenshots of every card face).
    pub fn pretend_device_state(&mut self, device: Id, state: &str) {
        use crate::hosting::State;
        self.hosting.pretend(
            device,
            match state {
                "loading" => State::Loading,
                "ready" => State::Ready,
                "failed" => State::Failed(
                    "Crashed while loading: access violation, exception 0xc0000005.".into(),
                ),
                "safe" => State::Safe,
                _ => State::Missing,
            },
        );
    }

    /// Gives a plugin device a program list, a chosen sound (changed or not)
    /// and preset files, without a host (screenshots of the preset menu).
    pub fn give_device_presets(
        &mut self,
        device: Id,
        programs: &[&str],
        chosen: &str,
        changed: bool,
        files: &[(Option<&str>, &str, bool)],
    ) {
        self.hosting.pretend_programs(device, "Factory", programs);
        let params = find_device(self.document.project(), device)
            .map(|(_, d)| d.params.clone())
            .unwrap_or_default();
        let baseline = if changed {
            let mut other = params.clone();
            other.insert(9999, 1.0);
            other
        } else {
            params
        };
        self.chain
            .presets
            .pretend(device, chosen, Some(baseline), files);
    }

    /// Opens the preset menu of a card (screenshots).
    pub fn open_preset_menu_of(&mut self, ctx: &egui::Context, device: Id) {
        self.open_preset_menu(ctx, device, false);
    }

    /// Says a plugin device failed `count` times just now and that its last
    /// good state is `age_seconds` old (screenshots of the failed card).
    pub fn pretend_failures(&mut self, device: Id, count: usize, age_seconds: Option<u64>) {
        self.hosting.pretend_failures(
            device,
            count,
            age_seconds.map(std::time::Duration::from_secs),
        );
    }

    /// Shows safe mode (and, when `offer`, its offer) without a host: for
    /// screenshots.
    pub fn pretend_safe_mode(&mut self, on: bool, offer: bool) {
        self.safe.on = on;
        self.safe.offer = offer;
        self.hosting.set_safe(on);
    }

    /// Gives a plugin device a picture of `width` by `height` pixels from
    /// `pixel(x, y)`, as if its bundle held one (screenshots of the card).
    pub fn give_device_picture(
        &mut self,
        ctx: &egui::Context,
        device: Id,
        (width, height): (usize, usize),
        pixel: impl Fn(usize, usize) -> [u8; 4],
    ) {
        let class = self
            .document
            .project()
            .tracks
            .iter()
            .flat_map(|t| t.devices.iter())
            .find(|d| d.id == device)
            .and_then(|d| match &d.plugin {
                lumen_project::model::PluginRef::Vst3 { class_id, .. } => Some(class_id.clone()),
                _ => None,
            });
        let Some(class) = class else { return };
        let mut rgba = Vec::with_capacity(width * height * 4);
        for y in 0..height {
            for x in 0..width {
                rgba.extend(pixel(x, y));
            }
        }
        let image = egui::ColorImage::from_rgba_unmultiplied([width, height], &rgba);
        let wake = ctx.clone();
        self.chain
            .pictures
            .get_or_insert_with(|| crate::chain::Pictures::new(move || wake.request_repaint()))
            .put(ctx, &class, image);
    }

    /// How a plugin device's own picture stands: `none`, `loading`, `absent`
    /// or `ready` (the card shows it when ready).
    pub fn device_picture(&self, device: Id) -> &'static str {
        let class = self
            .document
            .project()
            .tracks
            .iter()
            .flat_map(|t| t.devices.iter())
            .find(|d| d.id == device)
            .and_then(|d| match &d.plugin {
                lumen_project::model::PluginRef::Vst3 { class_id, .. } => Some(class_id.clone()),
                _ => None,
            });
        match (class, &self.chain.pictures) {
            (Some(class), Some(pictures)) => pictures.state(&class),
            _ => "none",
        }
    }

    /// The latency a plugin device last reported, in frames.
    pub fn device_latency(&self, device: Id) -> Option<u32> {
        self.hosting.latency(device)
    }

    /// Starts a failed device's host again, as its card's Reload will.
    pub fn reload_device(&mut self, device: Id) {
        self.hosting.reload(device);
    }

    /// Starts a failed device's host again with no sound of its own, as its
    /// card's "Reload without sound" will.
    pub fn reload_device_clean(&mut self, device: Id) {
        self.hosting.reload_clean(device);
    }

    /// Ends a device's host as a crash would leave it (the user ending a plugin
    /// that stopped answering).
    pub fn end_device_host(&mut self, device: Id) {
        self.hosting.end_plugin(device);
    }

    /// How often running plugins are read for a checkpoint.
    pub fn set_checkpoint_every(&mut self, every: Duration) {
        self.hosting.set_checkpoint_every(every);
    }

    /// Whether a device's plugin has stopped three times in two minutes.
    pub fn device_keeps_failing(&self, device: Id) -> bool {
        self.hosting.keeps_failing(device)
    }

    /// How long ago a device's last good state was read, if one was.
    pub fn device_last_good_age(&self, device: Id) -> Option<Duration> {
        self.hosting.last_good_age(device)
    }

    /// Removes a device from a track, as the interface will (one undo step).
    pub fn remove_device(&mut self, track: Id, device: Id) {
        let _ = self.document.remove_device(track, device);
    }

    /// Folds a card to a strip, or opens it again (a view, not an edit).
    pub fn fold_device(&mut self, device: Id) {
        self.toggle_fold_of(device);
    }

    /// Moves a device to a track: right after `after` on it, or at the end.
    /// A drag will do this (one undo step).
    pub fn move_device(&mut self, from: Id, device: Id, to: Id, after: Option<Id>) {
        let slot = after.map_or(crate::document::Slot::End, crate::document::Slot::After);
        let _ = self.document.move_device(from, device, to, slot);
    }

    /// Copies a device to a track, placed like [`Self::move_device`]; returns
    /// the copy's id.
    pub fn copy_device(&mut self, from: Id, device: Id, to: Id, after: Option<Id>) -> Option<Id> {
        let slot = after.map_or(crate::document::Slot::End, crate::document::Slot::After);
        self.document.copy_device(from, device, to, slot).ok()
    }

    /// Sets a plugin's parameter (normalized) as the interface's controls will:
    /// the project holds the value, and one drag is one undo step.
    pub fn set_device_param(&mut self, device: Id, id: u32, value: f64) {
        self.document.override_param(device, id);
        self.set_device_param_at_rest(device, id, value);
    }

    /// Sets the value the project holds for a plugin's parameter without
    /// moving a control: what a loaded program or a patch does. A lane that
    /// drives the parameter is not taken over.
    pub fn set_device_param_at_rest(&mut self, device: Id, id: u32, value: f64) {
        let _ = self.document.set_device_params(
            device,
            &[(id, value)],
            "Change parameter",
            Some("test-parameter"),
        );
    }

    /// The programs a device's plugin offers: each list's name and its
    /// programs' names.
    pub fn device_programs(&self, device: Id) -> Vec<(String, Vec<String>)> {
        self.hosting
            .programs(device)
            .iter()
            .map(|l| (l.name.clone(), l.programs.clone()))
            .collect()
    }

    /// Chooses a program as the card's preset menu will.
    pub fn choose_program(&mut self, device: Id, list: usize, program: usize) {
        self.load_program(device, list, program);
    }

    /// Loads a preset file's bytes into a device's plugin as the card's preset
    /// menu will; what the file is wrong with is the error.
    pub fn load_preset_from_bytes(
        &mut self,
        device: Id,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let preset = lumen_hosting::Preset::parse(bytes).map_err(|e| e.to_string())?;
        self.load_preset(device, name, preset);
        Ok(())
    }

    /// The parameter values the project holds for a device, by id.
    pub fn device_params(&self, device: Id) -> Vec<(u32, f64)> {
        crate::document::find_device(self.document.project(), device)
            .map(|(_, d)| d.params.iter().map(|(&k, &v)| (k, v)).collect())
            .unwrap_or_default()
    }

    /// Gives a device a parameter list as if its plugin had listed it, for
    /// screenshots of the card's strip: `(title, value, text)` each.
    pub fn give_device_parameters(&mut self, device: Id, list: &[(&str, f64, &str)]) {
        self.hosting.pretend_parameters(device, list);
    }

    /// Gives a device stepped parameters after those it has (screenshots of
    /// the switches and choosers): `(title, steps, value, choices)` each.
    pub fn give_device_stepped(&mut self, device: Id, list: &[(&str, i32, f64, &[&str])]) {
        self.hosting.pretend_stepped(device, list);
    }

    /// The parameter ids on a device's card strip, in order.
    pub fn strip_parameters(&self, device: Id) -> Vec<u32> {
        self.strip_ids(device)
    }

    /// The plugin's words for each step of a stepped parameter, as far as it
    /// has said them.
    pub fn device_choices(&self, device: Id, parameter: u32) -> Vec<String> {
        self.hosting.choices(device, parameter).to_vec()
    }

    /// Puts exactly these parameters on the device's strip, as a person does
    /// from the panel.
    pub fn show_on_strip(&mut self, device: Id, ids: &[u32]) {
        self.chain.shown.insert(device, ids.to_vec());
    }

    /// Sends a parameter value to the device's plugin as a control in its own
    /// window would, with no edit of the project.
    pub fn poke_plugin(&mut self, device: Id, id: u32, value: f64) {
        self.hosting.poke_param(device, id, value);
    }

    /// How many values Lumen has sent the device's plugin to match the project.
    pub fn values_sent_to(&self, device: Id) -> u32 {
        self.hosting.values_sent(device)
    }

    /// Whether the device's plugin window is open but hidden because the
    /// Devices tab shows another track.
    pub fn editor_hidden(&self, device: Id) -> bool {
        self.hosting.editor_hidden(device)
    }

    /// Where a plugin's window is remembered to open, by the device's class:
    /// `(x, y, width, height)`.
    pub fn editor_place(&self, device: Id) -> Option<(i32, i32, u32, u32)> {
        let class = self.hosting.class(device)?;
        let p = self.settings.plugins.editors.get(class)?;
        Some((p.x, p.y, p.width, p.height))
    }

    /// Remembers a window place for the device's plugin, as if it had been
    /// left there.
    pub fn set_editor_place(&mut self, device: Id, place: (i32, i32, u32, u32)) {
        if let Some(class) = self.hosting.class(device).map(str::to_owned) {
            self.settings.plugins.remember_editor(
                &class,
                crate::plugins::EditorPlace {
                    x: place.0,
                    y: place.1,
                    width: place.2,
                    height: place.3,
                    dpi: 96,
                },
            );
        }
    }

    /// Sets how a device's plugin window looks on its card, for screenshots:
    /// `open`, `opening`, `stuck`, `none` (no editor) or `closed`; `learn`
    /// also turns Learn on.
    pub fn give_device_editor(&mut self, device: Id, state: &str, learn: bool) {
        use crate::hosting::EditorState;
        let state = match state {
            "open" => EditorState::Open,
            "opening" => EditorState::Opening,
            "stuck" => EditorState::NotResponding,
            "none" => EditorState::Refused("This plugin has no editor.".into()),
            _ => EditorState::Closed,
        };
        self.hosting.pretend_editor(device, state);
        if learn {
            self.chain.learning = Some(device);
        }
    }

    /// Whether Learn is on for the device.
    pub fn learning(&self, device: Id) -> bool {
        self.chain.learning == Some(device)
    }

    /// How many changed plugins could not be read for the save.
    pub fn unread_plugins(&self) -> usize {
        self.hosting.unread()
    }

    /// A parameter's value as far as the host bridge knows it.
    pub fn device_param_known(&self, device: Id, id: u32) -> Option<f64> {
        self.hosting.value(device, id)
    }

    /// The titles of a running plugin's parameters, in its order.
    pub fn device_param_titles(&self, device: Id) -> Vec<String> {
        self.hosting
            .parameters(device)
            .iter()
            .map(|p| p.title.clone())
            .collect()
    }

    /// Bypasses or turns on a device, as the interface will.
    pub fn set_device_bypass(&mut self, track: Id, device: Id, bypass: bool) {
        let _ = self.document.set_device_bypass(track, device, bypass);
    }

    /// The delay compensation the engine was last sent, in frames.
    pub fn compensation(&self) -> u32 {
        self.transport.audio.mixer.compensation().frames
    }

    /// A scan, or the first read of the plugin list, is under way.
    pub fn plugins_busy(&self) -> bool {
        self.plugins.busy()
    }

    /// What the browser's scan line says is happening, in words.
    pub fn plugin_status(&self) -> String {
        format!("{:?}", self.plugins.status())
    }

    /// The installed plugins (not Lumen's own devices), by name.
    pub fn plugin_names(&self) -> Vec<String> {
        self.plugins
            .catalog()
            .devices()
            .iter()
            .filter(|d| d.source == lumen_plugin_catalog::catalog::Source::Vst3)
            .map(|d| d.name.clone())
            .collect()
    }

    /// The modules Lumen will not load, by name.
    pub fn blocked_plugins(&self) -> Vec<String> {
        self.plugins
            .catalog()
            .blocked()
            .iter()
            .map(|b| b.name.clone())
            .collect()
    }

    /// The ids of the favorite devices, as the settings keep them.
    pub fn favorite_devices(&self) -> Vec<String> {
        self.settings.plugins.favorites.clone()
    }

    /// Selects these tracks, as clicking their headers would.
    pub fn select_tracks(&mut self, ids: &[Id]) {
        self.arrangement.selected = ids.to_vec();
    }

    /// Chooses a card in the Devices tab, as clicking it would.
    pub fn choose_card(&mut self, device: Id) {
        self.chain.selected = Some(device);
    }

    /// The card chosen in the Devices tab, if one is.
    pub fn chosen_card(&self) -> Option<Id> {
        self.chain.selected
    }

    /// The ids of the recently chosen devices, most recent first.
    pub fn recent_devices(&self) -> Vec<String> {
        self.settings.plugins.recents.clone()
    }

    /// The id of the device selected in the browser, if one is.
    pub fn selected_device(&self) -> Option<String> {
        match &self.browser.view.selected {
            Some(crate::browser::view::Selection::Device(id)) => Some(id.clone()),
            _ => None,
        }
    }

    /// Whether the browser pane is shown.
    pub fn browser_shown(&self) -> bool {
        self.show_browser
    }
}
