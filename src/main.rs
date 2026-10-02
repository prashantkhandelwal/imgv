use std::collections::VecDeque;
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};

use ::image::ImageReader;
use iced::widget::{button, column, container, horizontal_space, image, row, scrollable, text};
use iced::{Alignment, Element, Length, Task, Theme};

const THUMBNAIL_WIDTH: u32 = 220;
const THUMBNAIL_HEIGHT: u32 = 150;
const GRID_COLUMNS: usize = 3;
const MAX_CONCURRENT_THUMBNAILS: usize = 4;
#[cfg(target_os = "macos")]
static NEXT_NATIVE_DECODE_ID: AtomicU64 = AtomicU64::new(0);

fn main() -> iced::Result {
    iced::application("imgv", App::update, App::view)
        .theme(|_| Theme::Light)
        .window_size((1280.0, 800.0))
        .run_with(|| (App::default(), Task::none()))
}

#[derive(Default)]
struct App {
    folder: Option<PathBuf>,
    images: Vec<ImageEntry>,
    selected: Option<PathBuf>,
    loading_folder: bool,
    error: Option<String>,
    thumbnail_queue: VecDeque<PathBuf>,
    active_thumbnail_loads: usize,
    folder_generation: u64,
}

struct ImageEntry {
    path: PathBuf,
    name: String,
    thumbnail: Thumbnail,
    repaired_preview: Option<image::Handle>,
}

enum Thumbnail {
    Loading,
    Ready(image::Handle),
    Failed(String),
}

#[derive(Debug, Clone)]
enum Message {
    ChooseFolder,
    FolderChosen(Option<PathBuf>),
    FolderScanned(u64, Result<Vec<PathBuf>, String>),
    ThumbnailLoaded(u64, PathBuf, Result<ThumbnailData, String>),
    SelectImage(PathBuf),
}

#[derive(Debug, Clone)]
struct ThumbnailData {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    repaired_preview: Option<Vec<u8>>,
}

impl App {
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::ChooseFolder => {
                self.error = None;
                Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Choose an image folder")
                            .pick_folder()
                            .await
                            .map(|folder| folder.path().to_path_buf())
                    },
                    Message::FolderChosen,
                )
            }
            Message::FolderChosen(Some(folder)) => {
                self.folder_generation = self.folder_generation.wrapping_add(1);
                let generation = self.folder_generation;
                self.folder = Some(folder.clone());
                self.images.clear();
                self.selected = None;
                self.loading_folder = true;
                self.error = None;
                self.thumbnail_queue.clear();
                self.active_thumbnail_loads = 0;

                Task::perform(async move { scan_folder(&folder) }, move |result| {
                    Message::FolderScanned(generation, result)
                })
            }
            Message::FolderChosen(None) => Task::none(),
            Message::FolderScanned(generation, _) if generation != self.folder_generation => {
                Task::none()
            }
            Message::FolderScanned(_, Ok(paths)) => {
                self.loading_folder = false;
                self.selected = paths.first().cloned();
                self.images = paths
                    .iter()
                    .map(|path| ImageEntry {
                        name: path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("Unknown image")
                            .to_owned(),
                        path: path.clone(),
                        thumbnail: Thumbnail::Loading,
                        repaired_preview: None,
                    })
                    .collect();
                self.thumbnail_queue = paths.into();

                self.start_thumbnail_loads()
            }
            Message::FolderScanned(_, Err(error)) => {
                self.loading_folder = false;
                self.error = Some(error);
                Task::none()
            }
            Message::ThumbnailLoaded(generation, _, _) if generation != self.folder_generation => {
                Task::none()
            }
            Message::ThumbnailLoaded(_, path, result) => {
                self.active_thumbnail_loads = self.active_thumbnail_loads.saturating_sub(1);
                if let Some(entry) = self.images.iter_mut().find(|entry| entry.path == path) {
                    entry.thumbnail = match result {
                        Ok(data) => {
                            entry.repaired_preview =
                                data.repaired_preview.map(image::Handle::from_bytes);
                            Thumbnail::Ready(image::Handle::from_rgba(
                                data.width,
                                data.height,
                                data.pixels,
                            ))
                        }
                        Err(error) => {
                            eprintln!("Could not load {}: {error}", path.display());
                            Thumbnail::Failed(error)
                        }
                    };
                }
                self.start_thumbnail_loads()
            }
            Message::SelectImage(path) => {
                self.selected = Some(path.clone());

                if let Some(entry) = self.images.iter_mut().find(|entry| entry.path == path)
                    && matches!(entry.thumbnail, Thumbnail::Failed(_))
                {
                    entry.thumbnail = Thumbnail::Loading;
                    self.thumbnail_queue.push_front(path);
                    return self.start_thumbnail_loads();
                }

                Task::none()
            }
        }
    }

    fn start_thumbnail_loads(&mut self) -> Task<Message> {
        let paths = take_thumbnail_batch(
            &mut self.thumbnail_queue,
            self.active_thumbnail_loads,
            MAX_CONCURRENT_THUMBNAILS,
        );
        self.active_thumbnail_loads += paths.len();
        let generation = self.folder_generation;

        Task::batch(paths.into_iter().map(move |path| {
            let result_path = path.clone();
            Task::perform(async move { load_thumbnail(&path) }, move |result| {
                Message::ThumbnailLoaded(generation, result_path.clone(), result)
            })
        }))
    }

    fn view(&self) -> Element<'_, Message> {
        let folder_name = self
            .folder
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("No folder selected");

        let header = row![
            column![text("imgv").size(28), text(folder_name).size(14)].spacing(2),
            horizontal_space(),
            button("Open folder")
                .on_press(Message::ChooseFolder)
                .padding([10, 16]),
        ]
        .align_y(Alignment::Center)
        .padding([14, 20]);

        let content: Element<'_, Message> = if self.loading_folder {
            centered_message("Finding images...")
        } else if let Some(error) = &self.error {
            centered_message(error)
        } else if self.folder.is_none() {
            centered_message("Choose a folder to browse its images")
        } else if self.images.is_empty() {
            centered_message("No supported images found")
        } else {
            row![self.thumbnail_panel(), self.preview_panel()]
                .height(Length::Fill)
                .into()
        };

        column![header, content].height(Length::Fill).into()
    }

    fn thumbnail_panel(&self) -> Element<'_, Message> {
        let mut grid = column![].spacing(12);

        for entries in self.images.chunks(GRID_COLUMNS) {
            let mut grid_row = row![].spacing(12);
            for entry in entries {
                grid_row = grid_row.push(self.thumbnail_tile(entry));
            }
            grid = grid.push(grid_row);
        }

        container(scrollable(grid).direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::default(),
        )))
        .width(Length::Fixed(560.0))
        .height(Length::Fill)
        .padding(16)
        .into()
    }

    fn thumbnail_tile<'a>(&self, entry: &'a ImageEntry) -> Element<'a, Message> {
        let visual: Element<'a, Message> = match &entry.thumbnail {
            Thumbnail::Ready(handle) => image(handle.clone())
                .width(Length::Fill)
                .height(Length::Fixed(112.0))
                .content_fit(iced::ContentFit::Cover)
                .into(),
            Thumbnail::Loading => container(text("Loading...").size(13))
                .center(Length::Fill)
                .height(112)
                .into(),
            Thumbnail::Failed(error) => container(
                column![
                    text("Unavailable").size(13),
                    text(compact_error(error)).size(10)
                ]
                .spacing(4)
                .align_x(Alignment::Center),
            )
            .center(Length::Fill)
            .height(112)
            .into(),
        };

        button(
            column![
                visual,
                text(&entry.name)
                    .size(13)
                    .width(Length::Fill)
                    .wrapping(text::Wrapping::None),
            ]
            .spacing(7)
            .width(Length::Fill),
        )
        .on_press(Message::SelectImage(entry.path.clone()))
        .width(Length::Fixed(168.0))
        .padding(7)
        .into()
    }

    fn preview_panel(&self) -> Element<'_, Message> {
        match &self.selected {
            Some(path) => {
                let repaired_preview = self
                    .images
                    .iter()
                    .find(|entry| entry.path == *path)
                    .and_then(|entry| entry.repaired_preview.clone());
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Image");

                column![
                    container(
                        image(repaired_preview.unwrap_or_else(|| image::Handle::from_path(path)),)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced::ContentFit::Contain),
                    )
                    .center(Length::Fill),
                    text(name).size(16),
                ]
                .spacing(12)
                .padding([16, 20])
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            }
            None => centered_message("Select an image"),
        }
    }
}

fn centered_message(message: &str) -> Element<'_, Message> {
    container(text(message).size(18))
        .center(Length::Fill)
        .into()
}

fn take_thumbnail_batch(
    queue: &mut VecDeque<PathBuf>,
    active: usize,
    limit: usize,
) -> Vec<PathBuf> {
    let available = limit.saturating_sub(active);
    queue
        .drain(..available.min(queue.len()))
        .collect::<Vec<_>>()
}

fn compact_error(error: &str) -> String {
    const MAX_LENGTH: usize = 48;
    let first_line = error.lines().next().unwrap_or("Decode failed");
    let mut characters = first_line.chars();
    let mut compact = characters.by_ref().take(MAX_LENGTH).collect::<String>();
    if characters.next().is_some() {
        compact.push_str("...");
    }
    compact
}

fn scan_folder(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let entries =
        std::fs::read_dir(folder).map_err(|error| format!("Could not open folder: {error}"))?;
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && is_supported_image(path))
        .collect::<Vec<_>>();

    paths.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(paths)
}

fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("avif" | "bmp" | "gif" | "ico" | "jpeg" | "jpg" | "png" | "tif" | "tiff" | "webp")
    )
}

fn load_thumbnail(path: &Path) -> Result<ThumbnailData, String> {
    let source = ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?
        .decode();
    let (source, repaired_preview) = match source {
        Ok(source) => (source, None),
        Err(original_error) => {
            let recovered = match recover_prefixed_jpeg(path)? {
                Some(recovered) => Some(recovered),
                None => decode_with_native_image_io(path)?,
            };
            recovered
                .map(|(source, bytes)| (source, Some(bytes)))
                .ok_or_else(|| {
                    format!("{original_error}; the system image decoder also rejected this file")
                })?
        }
    };
    let thumbnail = source
        .thumbnail(THUMBNAIL_WIDTH, THUMBNAIL_HEIGHT)
        .to_rgba8();
    let (width, height) = thumbnail.dimensions();

    Ok(ThumbnailData {
        width,
        height,
        pixels: thumbnail.into_raw(),
        repaired_preview,
    })
}

fn recover_prefixed_jpeg(path: &Path) -> Result<Option<(::image::DynamicImage, Vec<u8>)>, String> {
    const JPEG_START: [u8; 3] = [0xFF, 0xD8, 0xFF];
    const MAX_PREFIX_LENGTH: usize = 64 * 1024;

    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let search_length = bytes.len().min(MAX_PREFIX_LENGTH + JPEG_START.len());
    let Some(offset) = bytes[..search_length]
        .windows(JPEG_START.len())
        .position(|window| window == JPEG_START)
    else {
        return Ok(None);
    };

    if offset == 0 {
        return Ok(None);
    }

    let repaired = bytes[offset..].to_vec();
    match ::image::load_from_memory_with_format(&repaired, ::image::ImageFormat::Jpeg) {
        Ok(source) => Ok(Some((source, repaired))),
        Err(_) => Ok(None),
    }
}

#[cfg(target_os = "macos")]
fn decode_with_native_image_io(
    path: &Path,
) -> Result<Option<(::image::DynamicImage, Vec<u8>)>, String> {
    let decode_id = NEXT_NATIVE_DECODE_ID.fetch_add(1, Ordering::Relaxed);
    let output_path = std::env::temp_dir().join(format!(
        "imgv-native-{}-{decode_id}.png",
        std::process::id()
    ));
    let output = std::process::Command::new("/usr/bin/sips")
        .args(["-s", "format", "png"])
        .arg(path)
        .arg("--out")
        .arg(&output_path)
        .output()
        .map_err(|error| format!("Could not start the system image decoder: {error}"))?;

    if !output.status.success() {
        let _ = std::fs::remove_file(output_path);
        return Ok(None);
    }

    let converted = std::fs::read(&output_path)
        .map_err(|error| format!("Could not read the converted image: {error}"));
    let _ = std::fs::remove_file(output_path);
    let converted = converted?;
    let source = ::image::load_from_memory_with_format(&converted, ::image::ImageFormat::Png)
        .map_err(|error| format!("Could not read the system-converted image: {error}"))?;

    Ok(Some((source, converted)))
}

#[cfg(not(target_os = "macos"))]
fn decode_with_native_image_io(
    _path: &Path,
) -> Result<Option<(::image::DynamicImage, Vec<u8>)>, String> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{
        compact_error, decode_with_native_image_io, is_supported_image, recover_prefixed_jpeg,
        take_thumbnail_batch,
    };
    use std::collections::VecDeque;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};

    #[test]
    fn recognizes_supported_extensions_case_insensitively() {
        assert!(is_supported_image(Path::new("photo.JPG")));
        assert!(is_supported_image(Path::new("graphic.webp")));
        assert!(!is_supported_image(Path::new("notes.txt")));
    }

    #[test]
    fn thumbnail_batch_respects_available_capacity() {
        let mut queue = (0..10)
            .map(|index| PathBuf::from(format!("{index}.jpg")))
            .collect::<VecDeque<_>>();

        let batch = take_thumbnail_batch(&mut queue, 2, 4);

        assert_eq!(batch.len(), 2);
        assert_eq!(queue.len(), 8);
    }

    #[test]
    fn thumbnail_error_is_kept_short() {
        let error = "a".repeat(80);
        assert_eq!(compact_error(&error).chars().count(), 51);
    }

    #[test]
    fn recovers_a_jpeg_with_leading_bytes() {
        let mut jpeg = Cursor::new(Vec::new());
        ::image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut jpeg, ::image::ImageFormat::Jpeg)
            .unwrap();
        let mut prefixed = vec![0x00, 0x05];
        prefixed.extend(jpeg.into_inner());

        let path = std::env::temp_dir().join(format!("imgv-prefixed-{}.jpg", std::process::id()));
        std::fs::write(&path, prefixed).unwrap();
        let recovered = recover_prefixed_jpeg(&path).unwrap().unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(recovered.0.width(), 2);
        assert!(recovered.1.starts_with(&[0xFF, 0xD8, 0xFF]));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_image_io_fallback_produces_a_png() {
        let path = std::env::temp_dir().join(format!("imgv-native-{}.jpg", std::process::id()));
        ::image::DynamicImage::new_rgb8(2, 2).save(&path).unwrap();

        let recovered = decode_with_native_image_io(&path).unwrap().unwrap();
        std::fs::remove_file(path).unwrap();

        assert_eq!(recovered.0.width(), 2);
        assert!(recovered.1.starts_with(&[0x89, b'P', b'N', b'G']));
    }
}
