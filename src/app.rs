use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui;

use crate::engine::{self, Control, CopyEvent, CopyJob, Progress};
use crate::fmt::{human_bytes, human_eta};
use crate::listing::{Entry, IndexItem, SortKey, compare_entries, compare_index};
use crate::selection::{Check, PlanItem, Selection};
use crate::text;

const MAX_ERRORS: usize = 500;
const INDENT: f32 = 18.0;
const ARROW: f32 = 16.0;
const RED: egui::Color32 = egui::Color32::LIGHT_RED;

enum Msg {
    LoginDone(Result<(), String>),
    Listed(String, Result<Vec<Entry>, String>),
    IndexBatch(u64, Vec<IndexItem>),
    IndexDone(u64, Result<Option<String>, String>),
    Size(Result<(u64, u64), String>),
    Copy(CopyEvent),
}

enum LoginState {
    Idle,
    Running,
    Failed(String),
}

enum NodeState {
    Loading,
    Loaded(Vec<Entry>),
    Failed(String),
}

enum IndexState {
    Idle,
    Running,
    Done(Option<String>),
    Failed(String),
}

enum SizeState {
    Unknown,
    Running,
    Done { files: u64, bytes: u64 },
    Failed(String),
}

enum CopyState {
    Idle,
    Running { index: usize, total: usize, name: String, progress: Progress },
    Finished(String),
}

enum Action {
    Toggle(String),
    Select { path: String, is_dir: bool, on: bool },
    Retry(String),
    Reveal(String),
}

enum Row<'a> {
    Entry { depth: usize, entry: &'a Entry },
    Match(&'a IndexItem),
    Loading(usize),
    Empty(usize),
    Failed { depth: usize, path: String, message: String },
}

fn detail(modified: Option<i64>, size: Option<u64>) -> String {
    let parts: Vec<String> = [modified.map(text::format_date), size.map(human_bytes)].into_iter().flatten().collect();
    parts.join(" · ")
}

fn describe(item: &PlanItem) -> String {
    match item {
        PlanItem::Folder { src, excludes, .. } if excludes.is_empty() => format!("{src} (carpeta completa)"),
        PlanItem::Folder { src, excludes, .. } => format!("{src} (carpeta, sin {} elementos)", excludes.len()),
        PlanItem::Files { src_dir, files, .. } => {
            let place = if src_dir.is_empty() { "la raíz" } else { src_dir.as_str() };
            format!("{} fichero(s) de {place}", files.len())
        }
    }
}

pub struct App {
    ctx: egui::Context,
    rclone: Option<PathBuf>,
    conf: PathBuf,
    logged_in: bool,
    login: LoginState,
    nodes: HashMap<String, NodeState>,
    expanded: HashSet<String>,
    sort: SortKey,
    query: String,
    index: Vec<IndexItem>,
    index_state: IndexState,
    index_run: u64,
    index_control: Control,
    scroll_to: Option<String>,
    selection: Selection,
    size: SizeState,
    dest: String,
    skip_existing: bool,
    transfers: u32,
    copy: CopyState,
    errors: Vec<String>,
    control: Control,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new(ctx: egui::Context) -> Self {
        Self::with(ctx, engine::find_rclone(), engine::config_path())
    }

    fn with(ctx: egui::Context, rclone: Option<PathBuf>, conf: PathBuf) -> Self {
        let (tx, rx) = channel();
        Self {
            ctx,
            rclone,
            logged_in: engine::is_logged_in(&conf),
            conf,
            login: LoginState::Idle,
            nodes: HashMap::new(),
            expanded: HashSet::new(),
            sort: SortKey::NameAsc,
            query: String::new(),
            index: Vec::new(),
            index_state: IndexState::Idle,
            index_run: 0,
            index_control: Control::default(),
            scroll_to: None,
            selection: Selection::default(),
            size: SizeState::Unknown,
            dest: String::new(),
            skip_existing: true,
            transfers: 4,
            copy: CopyState::Idle,
            errors: Vec::new(),
            control: Control::default(),
            tx,
            rx,
        }
    }

    fn is_copying(&self) -> bool {
        matches!(self.copy, CopyState::Running { .. })
    }

    fn resort(&mut self) {
        let key = self.sort;
        for node in self.nodes.values_mut() {
            if let NodeState::Loaded(entries) = node {
                entries.sort_by(|a, b| compare_entries(key, a, b));
            }
        }
    }

    fn refresh(&mut self) {
        self.nodes.clear();
        self.index_control.cancel();
        self.index_run += 1;
        self.index.clear();
        self.index_state = IndexState::Idle;
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::LoginDone(Ok(())) => {
                self.logged_in = true;
                self.login = LoginState::Idle;
                self.refresh();
            }
            Msg::LoginDone(Err(e)) => {
                self.login = if e == "Cancelado" { LoginState::Idle } else { LoginState::Failed(e) };
            }
            Msg::Listed(path, Ok(mut entries)) => {
                let key = self.sort;
                entries.sort_by(|a, b| compare_entries(key, a, b));
                self.nodes.insert(path, NodeState::Loaded(entries));
            }
            Msg::Listed(path, Err(e)) => {
                self.nodes.insert(path, NodeState::Failed(e));
            }
            Msg::IndexBatch(run, batch) => {
                if run == self.index_run {
                    self.index.extend(batch);
                }
            }
            Msg::IndexDone(run, result) => {
                if run == self.index_run {
                    self.index_state = match result {
                        Ok(note) => IndexState::Done(note),
                        Err(e) if e == "Cancelado" => IndexState::Idle,
                        Err(e) => IndexState::Failed(e),
                    };
                }
            }
            Msg::Size(Ok((files, bytes))) => self.size = SizeState::Done { files, bytes },
            Msg::Size(Err(e)) => self.size = SizeState::Failed(e),
            Msg::Copy(CopyEvent::Start { index, total, name }) => {
                self.copy = CopyState::Running { index, total, name, progress: Progress::default() };
            }
            Msg::Copy(CopyEvent::Stats(p)) => {
                if let CopyState::Running { progress, .. } = &mut self.copy {
                    *progress = p;
                }
            }
            Msg::Copy(CopyEvent::Error(e)) => {
                if self.errors.len() < MAX_ERRORS {
                    self.errors.push(e);
                }
            }
            Msg::Copy(CopyEvent::Finished { cancelled, failed }) => {
                self.copy = CopyState::Finished(if cancelled {
                    "Copia cancelada. Lo ya copiado se queda en el destino.".to_string()
                } else if failed.is_empty() {
                    "Copia terminada sin errores.".to_string()
                } else {
                    format!("Copia terminada con errores en: {}", failed.join(", "))
                });
            }
        }
    }

    fn start_login(&mut self) {
        let Some(rclone) = self.rclone.clone() else { return };
        self.login = LoginState::Running;
        self.control.reset();
        let (conf, ctl, tx, ctx) = (self.conf.clone(), self.control.clone(), self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(Msg::LoginDone(engine::login(&rclone, &conf, &ctl)));
            ctx.request_repaint();
        });
    }

    fn logout(&mut self) {
        let _ = std::fs::remove_file(&self.conf);
        self.logged_in = false;
        self.refresh();
        self.expanded.clear();
        self.selection.clear();
        self.query.clear();
        self.size = SizeState::Unknown;
        self.copy = CopyState::Idle;
    }

    fn ensure_loaded(&mut self, path: &str) {
        if self.nodes.contains_key(path) {
            return;
        }
        let Some(rclone) = self.rclone.clone() else { return };
        self.nodes.insert(path.to_string(), NodeState::Loading);
        let (conf, tx, ctx, path) = (self.conf.clone(), self.tx.clone(), self.ctx.clone(), path.to_string());
        thread::spawn(move || {
            let result = engine::list_entries(&rclone, &conf, &path);
            let _ = tx.send(Msg::Listed(path, result));
            ctx.request_repaint();
        });
    }

    fn ensure_index(&mut self) {
        if !matches!(self.index_state, IndexState::Idle) {
            return;
        }
        let Some(rclone) = self.rclone.clone() else { return };
        self.index_run += 1;
        self.index.clear();
        self.index_state = IndexState::Running;
        self.index_control = Control::default();
        let run = self.index_run;
        let (conf, ctl, tx, ctx) = (self.conf.clone(), self.index_control.clone(), self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let result = engine::index_dirs(&rclone, &conf, &ctl, &|batch| {
                let _ = tx.send(Msg::IndexBatch(run, batch));
                ctx.request_repaint();
            });
            let _ = tx.send(Msg::IndexDone(run, result));
            ctx.request_repaint();
        });
    }

    fn start_size(&mut self, plan: Vec<PlanItem>) {
        let Some(rclone) = self.rclone.clone() else { return };
        self.size = SizeState::Running;
        let (conf, tx, ctx) = (self.conf.clone(), self.tx.clone(), self.ctx.clone());
        thread::spawn(move || {
            let _ = tx.send(Msg::Size(engine::selection_size(&rclone, &conf, &plan, &Control::default())));
            ctx.request_repaint();
        });
    }

    fn start_copy(&mut self) {
        let Some(rclone) = self.rclone.clone() else { return };
        let dest = PathBuf::from(self.dest.trim());
        if !dest.is_dir() {
            self.copy = CopyState::Finished(format!("La carpeta de destino no existe: {}", dest.display()));
            return;
        }
        let plan = self.selection.plan();
        self.errors.clear();
        self.control.reset();
        self.copy = CopyState::Running { index: 0, total: plan.len(), name: String::new(), progress: Progress::default() };
        let (conf, ctl, tx, ctx) = (self.conf.clone(), self.control.clone(), self.tx.clone(), self.ctx.clone());
        let job = CopyJob { plan, dest, skip_existing: self.skip_existing, transfers: self.transfers };
        thread::spawn(move || {
            engine::run_copy(&rclone, &conf, &job, &ctl, &|event| {
                let _ = tx.send(Msg::Copy(event));
                ctx.request_repaint();
            });
        });
    }

    fn reveal(&mut self, path: &str) {
        self.query.clear();
        let mut ancestor = String::new();
        for part in path.split('/') {
            if !ancestor.is_empty() {
                ancestor.push('/');
            }
            ancestor.push_str(part);
            self.expanded.insert(ancestor.clone());
        }
        self.scroll_to = Some(path.to_string());
    }

    fn apply(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Toggle(path) => {
                    if !self.expanded.remove(&path) {
                        self.expanded.insert(path);
                    }
                }
                Action::Select { path, is_dir, on } => {
                    self.selection.set(&path, is_dir, on);
                    self.size = SizeState::Unknown;
                }
                Action::Retry(path) => {
                    self.nodes.remove(&path);
                }
                Action::Reveal(path) => self.reveal(&path),
            }
        }
    }

    fn remove_from_selection(&mut self, item: &PlanItem) {
        match item {
            PlanItem::Folder { src, .. } => self.selection.set(src, true, false),
            PlanItem::Files { src_dir, files, .. } => {
                for file in files {
                    let path = if src_dir.is_empty() { file.clone() } else { format!("{src_dir}/{file}") };
                    self.selection.set(&path, false, false);
                }
            }
        }
        self.size = SizeState::Unknown;
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Copiar de OneDrive");
            ui.separator();
            if self.logged_in {
                ui.label("Sesión iniciada");
                if ui.add_enabled(!self.is_copying(), egui::Button::new("Cerrar sesión")).clicked() {
                    self.logout();
                }
            } else {
                ui.label("Sin sesión");
            }
        });
    }

    fn missing_rclone(&mut self, ui: &mut egui::Ui) {
        ui.heading("No encuentro rclone");
        ui.label("Esta aplicación usa rclone para hablar con OneDrive. Instálalo y pulsa «Volver a comprobar»:");
        ui.monospace("macOS:    brew install rclone");
        ui.monospace("Windows:  winget install Rclone.Rclone");
        ui.label("También puedes copiar el ejecutable de rclone junto a esta aplicación, o indicarlo a mano.");
        ui.horizontal(|ui| {
            if ui.button("Volver a comprobar").clicked() {
                self.rclone = engine::find_rclone();
            }
            if ui.button("Elegir rclone...").clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                self.rclone = Some(path);
            }
        });
    }

    fn login_screen(&mut self, ui: &mut egui::Ui) {
        ui.heading("Inicia sesión en OneDrive");
        ui.label("Se abrirá el navegador para que autorices el acceso con tu cuenta de Microsoft.");
        ui.label("La aplicación solo pide permiso de lectura: no puede modificar ni borrar nada en OneDrive.");
        ui.add_space(8.0);
        match &self.login {
            LoginState::Running => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Esperando a que termines en el navegador...");
                });
                if ui.button("Cancelar").clicked() {
                    self.control.cancel();
                }
            }
            state => {
                if let LoginState::Failed(e) = state {
                    ui.colored_label(RED, format!("No se pudo iniciar sesión: {e}"));
                }
                if ui.button("Iniciar sesión").clicked() {
                    self.start_login();
                }
            }
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Carpetas y ficheros");
            if ui.small_button("Actualizar").clicked() {
                self.refresh();
            }
        });
        ui.horizontal(|ui| {
            let label = ui.label("Buscar:");
            ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("carpeta en todo OneDrive...")
                    .desired_width((ui.available_width() - 34.0).max(80.0)),
            )
            .labelled_by(label.id);
            if ui.add_enabled(!self.query.is_empty(), egui::Button::new("x")).on_hover_text("Borrar la búsqueda").clicked() {
                self.query.clear();
            }
        });
        ui.horizontal(|ui| {
            let label = ui.label("Ordenar por:");
            let before = self.sort;
            let combo = egui::ComboBox::from_id_salt("sort").selected_text(self.sort.label()).show_ui(ui, |ui| {
                for key in SortKey::ALL {
                    ui.selectable_value(&mut self.sort, key, key.label());
                }
            });
            combo.response.labelled_by(label.id);
            if self.sort != before {
                self.resort();
            }
        });
    }

    fn index_status(&mut self, ui: &mut egui::Ui, results: usize) {
        match &self.index_state {
            IndexState::Running => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak(format!("Indexando carpetas... {} encontradas · {results} resultados", self.index.len()));
                });
            }
            IndexState::Done(note) => {
                ui.weak(format!("{results} resultados de {} carpetas indexadas", self.index.len()));
                if let Some(note) = note {
                    ui.weak(note);
                }
            }
            IndexState::Failed(e) => {
                ui.colored_label(RED, format!("No se pudo indexar: {e}"));
                if ui.small_button("Reintentar").clicked() {
                    self.index_state = IndexState::Idle;
                }
            }
            IndexState::Idle => {}
        }
    }

    fn tree_rows<'a>(&'a self, to_load: &mut Vec<String>) -> Vec<Row<'a>> {
        let mut rows = Vec::new();
        self.push_dir("", 0, &mut rows, to_load);
        rows
    }

    fn push_dir<'a>(&'a self, path: &str, depth: usize, rows: &mut Vec<Row<'a>>, to_load: &mut Vec<String>) {
        match self.nodes.get(path) {
            None => {
                to_load.push(path.to_string());
                rows.push(Row::Loading(depth));
            }
            Some(NodeState::Loading) => rows.push(Row::Loading(depth)),
            Some(NodeState::Failed(message)) => {
                rows.push(Row::Failed { depth, path: path.to_string(), message: message.clone() });
            }
            Some(NodeState::Loaded(entries)) if entries.is_empty() => rows.push(Row::Empty(depth)),
            Some(NodeState::Loaded(entries)) => {
                for entry in entries {
                    rows.push(Row::Entry { depth, entry });
                    if entry.is_dir && self.expanded.contains(&entry.path) {
                        self.push_dir(&entry.path, depth + 1, rows, to_load);
                    }
                }
            }
        }
    }

    fn search_rows(&self) -> Vec<Row<'_>> {
        let query = text::fold(&self.query);
        let mut items: Vec<&IndexItem> = self.index.iter().filter(|i| text::matches_all_words(&i.folded_path, &query)).collect();
        items.sort_by(|a, b| compare_index(self.sort, a, b));
        items.into_iter().map(Row::Match).collect()
    }

    fn check_box(&self, ui: &mut egui::Ui, path: &str, is_dir: bool, label: &str, actions: &mut Vec<Action>) {
        let check = self.selection.check(path);
        let mut on = check == Check::On;
        if ui.add(egui::Checkbox::new(&mut on, label).indeterminate(check == Check::Partial)).changed() {
            actions.push(Action::Select { path: path.to_string(), is_dir, on });
        }
    }

    fn draw_row(&self, ui: &mut egui::Ui, row: &Row<'_>, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| match row {
            Row::Loading(depth) => {
                ui.add_space(*depth as f32 * INDENT + ARROW);
                ui.spinner();
                ui.weak("Cargando...");
            }
            Row::Empty(depth) => {
                ui.add_space(*depth as f32 * INDENT + ARROW);
                ui.weak("(vacía)");
            }
            Row::Failed { depth, path, message } => {
                ui.add_space(*depth as f32 * INDENT + ARROW);
                ui.colored_label(RED, message);
                if ui.small_button("Reintentar").clicked() {
                    actions.push(Action::Retry(path.clone()));
                }
            }
            Row::Entry { depth, entry } => {
                ui.add_space(*depth as f32 * INDENT);
                if entry.is_dir {
                    let open = self.expanded.contains(&entry.path);
                    let (_, response) = ui.allocate_exact_size(egui::vec2(ARROW, ARROW), egui::Sense::click());
                    egui::collapsing_header::paint_default_icon(ui, if open { 1.0 } else { 0.0 }, &response);
                    if response.clicked() {
                        actions.push(Action::Toggle(entry.path.clone()));
                    }
                } else {
                    ui.add_space(ARROW + ui.spacing().item_spacing.x);
                }
                self.check_box(ui, &entry.path, entry.is_dir, &entry.name, actions);
                ui.weak(detail(entry.modified, entry.size));
            }
            Row::Match(item) => {
                self.check_box(ui, &item.path, true, &item.name, actions);
                let parent = item.parent();
                ui.weak(format!("{} · {}", if parent.is_empty() { "raíz" } else { parent }, detail(item.modified, None)));
                if ui.small_button("Abrir").on_hover_text("Mostrar en el árbol").clicked() {
                    actions.push(Action::Reveal(item.path.clone()));
                }
            }
        });
    }

    fn tree_panel(&mut self, ui: &mut egui::Ui) {
        self.toolbar(ui);
        let searching = !self.query.trim().is_empty();
        if searching {
            self.ensure_index();
            let results = self.search_rows().len();
            self.index_status(ui, results);
        }
        ui.separator();

        let wanted = self.scroll_to.clone();
        let (mut actions, mut to_load) = (Vec::new(), Vec::new());
        let (found, still_loading) = {
            let rows = if searching { self.search_rows() } else { self.tree_rows(&mut to_load) };
            let found = wanted.as_ref().and_then(|w| rows.iter().position(|r| matches!(r, Row::Entry { entry, .. } if &entry.path == w)));
            let still_loading = rows.iter().any(|r| matches!(r, Row::Loading(_)));
            let row_height = ui.spacing().interact_size.y;
            let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(index) = found {
                area = area.vertical_scroll_offset((index as f32 * (row_height + ui.spacing().item_spacing.y) - 40.0).max(0.0));
            }
            area.show_rows(ui, row_height, rows.len(), |ui, range| {
                for row in &rows[range] {
                    self.draw_row(ui, row, &mut actions);
                }
            });
            (found, still_loading)
        };
        if found.is_some() || (wanted.is_some() && !still_loading) {
            self.scroll_to = None;
        }
        for path in to_load {
            self.ensure_loaded(&path);
        }
        self.apply(actions);
    }

    fn selection_panel(&mut self, ui: &mut egui::Ui, plan: &[PlanItem]) {
        ui.heading("Selección");
        if plan.is_empty() {
            ui.label("Marca carpetas o ficheros en la lista. Al marcar una carpeta se incluye todo lo que contiene; puedes desmarcar lo que no quieras.");
        }
        let mut remove = Vec::new();
        egui::ScrollArea::vertical().id_salt("plan").max_height(170.0).auto_shrink([false, true]).show(ui, |ui| {
            for item in plan {
                ui.horizontal(|ui| {
                    if ui.small_button("Quitar").clicked() {
                        remove.push(item.clone());
                    }
                    ui.label(describe(item));
                });
            }
        });
        for item in remove {
            self.remove_from_selection(&item);
        }

        let calculating = matches!(self.size, SizeState::Running);
        ui.horizontal(|ui| {
            if ui.add_enabled(!plan.is_empty() && !calculating, egui::Button::new("Calcular tamaño")).clicked() {
                self.start_size(plan.to_vec());
            }
            match &self.size {
                SizeState::Unknown => {}
                SizeState::Running => {
                    ui.spinner();
                    ui.weak("Calculando...");
                }
                SizeState::Done { files, bytes } => {
                    ui.label(format!("{files} ficheros · {}", human_bytes(*bytes)));
                }
                SizeState::Failed(e) => {
                    ui.colored_label(RED, e);
                }
            }
        });
    }

    fn copy_panel(&mut self, ui: &mut egui::Ui) {
        let plan = self.selection.plan();
        self.selection_panel(ui, &plan);
        ui.separator();

        ui.horizontal(|ui| {
            let label = ui.label("Destino:");
            ui.add(egui::TextEdit::singleline(&mut self.dest).desired_width((ui.available_width() - 110.0).max(100.0)))
                .labelled_by(label.id);
            if ui.button("Elegir...").clicked()
                && let Some(dir) = rfd::FileDialog::new().pick_folder()
            {
                self.dest = dir.display().to_string();
            }
        });
        ui.checkbox(&mut self.skip_existing, "No sobrescribir los ficheros que ya existan en el destino");
        ui.horizontal(|ui| {
            let label = ui.label("Descargas simultáneas:");
            ui.add(egui::DragValue::new(&mut self.transfers).range(1..=16)).labelled_by(label.id);
            ui.weak("(4 por defecto; más no siempre acelera, OneDrive limita el ritmo)");
        });
        ui.label("Cada carpeta se copia dentro del destino, con su mismo nombre. Nunca se borra nada.");
        ui.add_space(6.0);

        let copying = self.is_copying();
        let can_copy = !copying && !plan.is_empty() && !self.dest.trim().is_empty();
        ui.horizontal(|ui| {
            if ui.add_enabled(can_copy, egui::Button::new("Copiar")).clicked() {
                self.start_copy();
            }
            if copying && ui.button("Cancelar").clicked() {
                self.control.cancel();
            }
        });
        ui.add_space(6.0);

        match &self.copy {
            CopyState::Idle => {}
            CopyState::Finished(summary) => {
                ui.label(summary);
            }
            CopyState::Running { index, total, name, progress } => {
                ui.label(format!("Elemento {} de {}: {}", index + 1, total, name));
                let fraction = if progress.total_bytes > 0 { progress.bytes as f32 / progress.total_bytes as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(fraction.clamp(0.0, 1.0)).show_percentage());
                let eta = progress.eta.map(|s| format!(" · quedan {}", human_eta(s))).unwrap_or_default();
                ui.label(format!(
                    "{} de {} · {}/s{}",
                    human_bytes(progress.bytes),
                    human_bytes(progress.total_bytes),
                    human_bytes(progress.speed as u64),
                    eta
                ));
                ui.label(format!("Ficheros: {} de {}", progress.files_done, progress.files_total));
                for current in &progress.current {
                    ui.weak(current);
                }
            }
        }

        if !self.errors.is_empty() {
            ui.separator();
            ui.colored_label(RED, format!("Errores ({})", self.errors.len()));
            egui::ScrollArea::vertical().id_salt("errors").auto_shrink([false, false]).show(ui, |ui| {
                for e in &self.errors {
                    ui.colored_label(RED, e);
                }
            });
        }
    }
}

impl App {
    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle(msg);
        }
    }

    fn show(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        if self.rclone.is_none() {
            egui::CentralPanel::default_margins().show(ui, |ui| self.missing_rclone(ui));
        } else if !self.logged_in {
            egui::CentralPanel::default_margins().show(ui, |ui| self.login_screen(ui));
        } else {
            egui::Panel::left("tree").resizable(true).default_size(460.0).min_size(300.0).show(ui, |ui| self.tree_panel(ui));
            egui::CentralPanel::default_margins().show(ui, |ui| self.copy_panel(ui));
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn entry(parent: &str, name: &str, is_dir: bool, size: Option<u64>, modified: i64) -> Entry {
        Entry::new(parent, name, is_dir, size, Some(modified))
    }

    fn app() -> App {
        let conf = std::env::temp_dir().join("onedrive-copier-ui-tests.conf");
        let mut app = App::with(egui::Context::default(), Some(PathBuf::from("/bin/true")), conf);
        app.logged_in = true;
        app.nodes.insert(
            String::new(),
            NodeState::Loaded(vec![
                entry("", "Imágenes", true, None, 100),
                entry("", "Documentos", true, None, 300),
                entry("", "notas.txt", false, Some(2048), 200),
            ]),
        );
        app.nodes.insert(
            "Imágenes".to_string(),
            NodeState::Loaded(vec![entry("Imágenes", "2015", true, None, 10), entry("Imágenes", "foto.jpg", false, Some(5_000_000), 20)]),
        );
        app.nodes.insert("Imágenes/2015".to_string(), NodeState::Loaded(Vec::new()));
        app.nodes.insert("Documentos".to_string(), NodeState::Failed("sin permiso".to_string()));
        app.resort();
        app
    }

    fn names(app: &App) -> Vec<String> {
        app.tree_rows(&mut Vec::new())
            .iter()
            .map(|row| match row {
                Row::Entry { depth, entry } => format!("{}{}", "  ".repeat(*depth), entry.name),
                Row::Loading(d) => format!("{}...", "  ".repeat(*d)),
                Row::Empty(d) => format!("{}(vacía)", "  ".repeat(*d)),
                Row::Failed { depth, message, .. } => format!("{}!{message}", "  ".repeat(*depth)),
                Row::Match(item) => item.path.clone(),
            })
            .collect()
    }

    fn harness(app: App) -> Harness<'static, App> {
        Harness::builder().with_size(egui::vec2(1100.0, 700.0)).build_ui_state(
            |ui, app: &mut App| {
                app.pump();
                app.show(ui);
            },
            app,
        )
    }

    #[test]
    fn tree_lists_folders_before_files_and_nests_expanded_ones() {
        let mut app = app();
        assert_eq!(names(&app), ["Documentos", "Imágenes", "notas.txt"]);
        app.expanded.insert("Imágenes".to_string());
        app.expanded.insert("Documentos".to_string());
        app.expanded.insert("Imágenes/2015".to_string());
        assert_eq!(
            names(&app),
            ["Documentos", "  !sin permiso", "Imágenes", "  2015", "    (vacía)", "  foto.jpg", "notas.txt"]
        );
    }

    #[test]
    fn expanding_an_unloaded_folder_asks_to_load_it() {
        let mut app = app();
        app.expanded.insert("Imágenes/2015".to_string());
        app.nodes.remove("Imágenes/2015");
        app.expanded.insert("Imágenes".to_string());
        let mut to_load = Vec::new();
        let rows = app.tree_rows(&mut to_load);
        assert_eq!(to_load, ["Imágenes/2015"]);
        assert!(rows.iter().any(|r| matches!(r, Row::Loading(2))));
    }

    #[test]
    fn changing_the_sort_reorders_every_loaded_folder() {
        let mut app = app();
        app.sort = SortKey::DateAsc;
        app.resort();
        assert_eq!(names(&app), ["Imágenes", "Documentos", "notas.txt"]);
        app.sort = SortKey::NameDesc;
        app.resort();
        assert_eq!(names(&app), ["Imágenes", "Documentos", "notas.txt"]);
        app.sort = SortKey::NameAsc;
        app.resort();
        assert_eq!(names(&app), ["Documentos", "Imágenes", "notas.txt"]);
    }

    #[test]
    fn search_matches_any_word_order_ignoring_accents_and_sorts_results() {
        let mut app = app();
        app.index = vec![
            IndexItem::new("Imágenes/2015/Toledo RAW", Some(5)),
            IndexItem::new("Imágenes/2015/Toledo", Some(9)),
            IndexItem::new("Imágenes/2016/Ojén", Some(1)),
        ];
        app.query = "RAW toledo".to_string();
        let found = |app: &App| app.search_rows().iter().filter_map(|r| if let Row::Match(i) = r { Some(i.name.clone()) } else { None }).collect::<Vec<_>>();
        assert_eq!(found(&app), ["Toledo RAW"]);
        app.query = "imagenes TOLEDO".to_string();
        assert_eq!(found(&app), ["Toledo", "Toledo RAW"]);
        app.sort = SortKey::DateAsc;
        assert_eq!(found(&app), ["Toledo RAW", "Toledo"]);
        app.query = "ojen".to_string();
        assert_eq!(found(&app), ["Ojén"]);
    }

    #[test]
    fn reveal_expands_every_ancestor_and_clears_the_search() {
        let mut app = app();
        app.query = "algo".to_string();
        app.reveal("A/B/C");
        assert!(app.query.is_empty());
        for path in ["A", "A/B", "A/B/C"] {
            assert!(app.expanded.contains(path), "{path}");
        }
        assert_eq!(app.scroll_to.as_deref(), Some("A/B/C"));
    }

    #[test]
    fn clicking_a_folder_checkbox_selects_everything_inside_and_a_child_makes_it_partial() {
        let mut app = app();
        app.expanded.insert("Imágenes".to_string());
        let mut harness = harness(app);
        harness.run();

        harness.get_by_label("Imágenes").click();
        harness.run();
        let selection = &harness.state().selection;
        assert_eq!(selection.check("Imágenes"), Check::On);
        assert_eq!(selection.check("Imágenes/foto.jpg"), Check::On);
        assert_eq!(selection.check("Imágenes/2015"), Check::On);
        assert_eq!(selection.check("Documentos"), Check::Off);

        harness.get_by_label("foto.jpg").click();
        harness.run();
        let selection = &harness.state().selection;
        assert_eq!(selection.check("Imágenes"), Check::Partial);
        assert_eq!(selection.check("Imágenes/foto.jpg"), Check::Off);
        assert_eq!(selection.check("Imágenes/2015"), Check::On);
        assert_eq!(
            selection.plan(),
            vec![PlanItem::Folder { src: "Imágenes".into(), dest: "Imágenes".into(), excludes: vec!["foto.jpg".into()] }]
        );

        harness.get_by_label("Imágenes").click();
        harness.run();
        assert_eq!(harness.state().selection.check("Imágenes"), Check::On);
        harness.get_by_label("Imágenes").click();
        harness.run();
        assert!(harness.state().selection.is_empty());
    }

    #[test]
    fn selecting_a_loose_file_shows_it_in_the_selection_panel_and_it_can_be_removed() {
        let mut harness = harness(app());
        harness.run();
        harness.get_by_label("notas.txt").click();
        harness.run();
        assert_eq!(harness.state().selection.plan().len(), 1);
        harness.get_by_label_contains("1 fichero(s) de la raíz").hover();
        harness.get_by_label("Quitar").click();
        harness.run();
        assert!(harness.state().selection.is_empty());
    }

    #[test]
    fn every_screen_state_draws_without_panicking() {
        let mut app = app();
        app.query = "imag".to_string();
        app.index = vec![IndexItem::new("Imágenes/2015", Some(5))];
        app.index_state = IndexState::Done(Some("Algunas carpetas no se pudieron listar: X".to_string()));
        app.selection.set("Imágenes", true, true);
        app.size = SizeState::Done { files: 3, bytes: 4_000_000 };
        app.copy = CopyState::Running {
            index: 0,
            total: 2,
            name: "Imágenes".to_string(),
            progress: Progress { bytes: 5, total_bytes: 10, speed: 1e6, eta: Some(90), files_done: 1, files_total: 2, current: vec!["a.jpg".to_string()] },
        };
        app.errors.push("x/y.jpg: Failed to copy".to_string());
        let mut screen = harness(app);
        screen.run_steps(3);
        screen.get_by_label("Quitar");
        screen.get_by_label_contains("Elemento 1 de 2");

        let mut missing = App::with(egui::Context::default(), None, std::env::temp_dir().join("onedrive-copier-ui-tests-2.conf"));
        missing.logged_in = false;
        let mut screen = harness(missing);
        screen.run_steps(2);
        screen.get_by_label("No encuentro rclone");

        let conf = std::env::temp_dir().join("onedrive-copier-ui-tests-3.conf");
        let mut logged_out = App::with(egui::Context::default(), Some(PathBuf::from("/bin/true")), conf);
        logged_out.logged_in = false;
        logged_out.login = LoginState::Failed("rechazado".to_string());
        let mut screen = harness(logged_out);
        screen.run_steps(2);
        screen.get_by_label_contains("rechazado");
    }
}
