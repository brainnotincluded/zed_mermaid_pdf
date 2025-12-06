use anyhow::{Context as _, Result};
use gpui::{
    actions, div, App, Context, Entity, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, Task, ViewContext, WeakEntity, Window,
    Model, img, ImageSource, ScrollView, list, ListState, ListAlignment, px,
};
use project::{Project, ProjectItem as _, ProjectPath, Worktree, ProjectEntryId};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::io::Cursor;
use workspace::{
    item::{Item, ItemEvent, ProjectItem},
    Workspace, Pane,
};
use pdfium_render::prelude::*;
use image::ImageFormat;

static PDFIUM: OnceLock<Option<Pdfium>> = OnceLock::new();

fn get_pdfium() -> Option<&'static Pdfium> {
    PDFIUM.get_or_init(|| {
        let bindings = Pdfium::bind_to_library(Pdfium::download_and_save_to_file().ok()?)
            .or_else(|_| Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path("./")))
            .ok()?;
        Some(Pdfium::new(bindings))
    }).as_ref()
}

actions!(pdf, [OpenPdf]);

pub fn init(cx: &mut App) {
    workspace::register_project_item::<PdfPreviewView>(cx);
}

pub struct PdfItem {
    path: ProjectPath,
}

impl project::ProjectItem for PdfItem {
    fn try_open(
        _project: &Entity<Project>,
        path: &ProjectPath,
        cx: &mut App,
    ) -> Option<Task<Result<Entity<Self>>>> {
        if !is_pdf_file(path) {
            return None;
        }

        let path = path.clone();
        Some(cx.spawn(|mut cx| async move {
            cx.update(|cx| {
                cx.new(|_| PdfItem { path })
            })
        }))
    }

    fn entry_id(&self, _cx: &App) -> Option<ProjectEntryId> {
        None
    }

    fn project_path(&self, _cx: &App) -> Option<ProjectPath> {
        Some(self.path.clone())
    }

    fn is_dirty(&self) -> bool {
        false
    }
}

pub struct PdfPreviewView {
    item: Entity<PdfItem>,
    project: Entity<Project>,
    focus_handle: FocusHandle,
    images: Option<Vec<Arc<gpui::Image>>>,
    list_state: ListState,
    load_task: Option<Task<()>>,
    failed: bool,
}

impl PdfPreviewView {
    pub fn new(
        item: Entity<PdfItem>,
        project: Entity<Project>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            item,
            project,
            focus_handle: cx.focus_handle(),
            images: None,
            list_state: ListState::new(0, ListAlignment::Top, px(1000.0)),
            load_task: None,
            failed: false,
        };
        this.load_pdf(cx);
        this
    }

    fn load_pdf(&mut self, cx: &mut Context<Self>) {
        let project = self.project.clone();
        let item = self.item.clone();
        
        self.load_task = Some(cx.spawn(|this, mut cx| async move {
             let path = item.read_with(&cx, |item, _| item.path.clone()).ok()?;
             
             let worktree = project.update(&mut cx, |project, cx| {
                 project.worktree_for_id(path.worktree_id, cx)
             }).ok().flatten();

             if let Some(worktree) = worktree {
                 let load_file = worktree.update(&mut cx, |worktree, cx| {
                     worktree.load_binary_file(path.path.as_ref(), cx)
                 });
                 
                 if let Ok(loaded_file) = load_file {
                     if let Ok(loaded) = loaded_file.await {
                         let bytes = loaded.content;
                         let images = cx.background_executor().spawn(async move {
                             render_pdf_bytes(&bytes)
                         }).await;
                         
                         this.update(&mut cx, |this, cx| {
                             if let Some(images) = images {
                                 this.list_state.reset(images.len());
                                 this.images = Some(images);
                             } else {
                                 this.failed = true;
                             }
                             cx.notify();
                         }).ok();
                     }
                 }
             }
             Some(())
        }));
    }
}

fn render_pdf_bytes(bytes: &[u8]) -> Option<Vec<Arc<gpui::Image>>> {
    let pdfium = get_pdfium()?;
    let document = pdfium.load_pdf_from_byte_slice(bytes, None).ok()?;
    
    let mut images = Vec::new();
    for page in document.pages() {
        let bitmap = page.render_with_config(&PdfRenderConfig::new().set_target_width(1000)).ok()?;
        let image = bitmap.as_image(); 
        
        let mut buffer = Cursor::new(Vec::new());
        image.write_to(&mut buffer, ImageFormat::Png).ok()?;
        images.push(Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, buffer.into_inner())));
    }
    Some(images)
}

fn is_pdf_file(path: &ProjectPath) -> bool {
    path.path
        .extension()
        .map_or(false, |ext| ext.eq_ignore_ascii_case("pdf"))
}

impl ProjectItem for PdfPreviewView {
    type Item = PdfItem;

    fn for_project_item(
        project: Entity<Project>,
        _pane: Option<&Pane>,
        item: Entity<Self::Item>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(item, project, cx)
    }
}

impl Render for PdfPreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(images) = &self.images {
            let images = images.clone();
            div()
                .size_full()
                .bg(cx.theme().colors().editor_background)
                .child(
                    list(self.list_state.clone(), move |ix, _, _| {
                        if let Some(image) = images.get(ix) {
                             div().p_4().child(img(image.clone()).max_w_full()).into_any()
                        } else {
                            div().into_any()
                        }
                    })
                    .size_full()
                )
        } else if self.failed {
             div()
                .flex()
                .size_full()
                .bg(cx.theme().colors().editor_background)
                .justify_center()
                .items_center()
                .child("Failed to load PDF. Ensure pdfium is available.")
        } else {
             div()
                .flex()
                .size_full()
                .bg(cx.theme().colors().editor_background)
                .justify_center()
                .items_center()
                .child("Loading PDF...")
        }
    }
}

impl EventEmitter<()> for PdfPreviewView {}

impl Focusable for PdfPreviewView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for PdfPreviewView {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        self.item.read(cx).path
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "PDF".to_string())
            .into()
    }

    fn to_item_events(_event: &Self::Event, _f: impl FnMut(ItemEvent)) {}
}
