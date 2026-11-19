use eframe::egui;
use std::io;
use std::path::PathBuf;
use std::process::Command;

const DOWNLOAD_PAGE: &str = "https://ffmpeg.org/download.html";

#[derive(Clone)]
pub(super) struct Ffmpeg(PathBuf);

impl Ffmpeg {
    #[cfg(test)]
    pub fn on_path() -> Self {
        Self("ffmpeg".into())
    }

    pub fn find() -> Result<Option<Self>, String> {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let name = if cfg!(windows) {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        };
        let system = std::env::split_paths(&path)
            .map(|dir| std::path::absolute(dir.join(name)))
            .filter_map(Result::ok)
            .filter(|exe| exe.is_file());
        Self::find_with(system, installed_copy())
    }

    fn find_with(
        system: impl IntoIterator<Item = PathBuf>,
        installed: Option<PathBuf>,
    ) -> Result<Option<Self>, String> {
        for path in system.into_iter().chain(installed) {
            let ffmpeg = Self(path);
            match ffmpeg.command().arg("-version").output() {
                Ok(output) if output.status.success() => return Ok(Some(ffmpeg)),
                Ok(output) => {
                    return Err(format!(
                        "Cannot check FFmpeg ({}): {}",
                        output.status,
                        String::from_utf8_lossy(&output.stderr).trim()
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(start_error(error)),
            }
        }
        Ok(None)
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.0);
        cmd.args(["-hide_banner", "-nostdin"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd
    }
}

pub(super) fn start_error(error: io::Error) -> String {
    if error.kind() == io::ErrorKind::NotFound {
        "FFmpeg was not found. Open the export dialog again to set it up.".into()
    } else {
        format!("Cannot start FFmpeg: {error}")
    }
}

pub(super) struct Setup {
    status: Result<Option<Ffmpeg>, String>,
    #[cfg(windows)]
    download: Option<windows::Download>,
    #[cfg(windows)]
    error: Option<String>,
}

impl Setup {
    pub fn new(status: Result<Option<Ffmpeg>, String>) -> Self {
        Self {
            status,
            #[cfg(windows)]
            download: None,
            #[cfg(windows)]
            error: None,
        }
    }

    pub fn executable(&self) -> Option<&Ffmpeg> {
        self.status.as_ref().ok().and_then(Option::as_ref)
    }

    pub fn show(&mut self, ui: &mut egui::Ui) {
        match &self.status {
            Ok(Some(_)) => {}
            Err(error) => {
                ui.colored_label(crate::palette::error(), error);
                ui.hyperlink_to("FFmpeg downloads", DOWNLOAD_PAGE);
            }
            Ok(None) => {
                #[cfg(windows)]
                self.offer_download(ui);
                #[cfg(not(windows))]
                {
                    ui.label("Install FFmpeg and make sure it is on hanstat’s PATH to export audiobooks.");
                    ui.hyperlink_to("FFmpeg downloads", DOWNLOAD_PAGE);
                }
            }
        }
    }

    #[cfg(windows)]
    pub fn poll(&mut self) {
        if let Some(download) = &self.download
            && let Some(result) = super::received(&download.receiver)
        {
            self.download = None;
            match result.flatten() {
                Ok(ffmpeg) => self.status = Ok(ffmpeg),
                Err(error) => self.error = Some(error),
            }
        }
    }

    #[cfg(windows)]
    fn offer_download(&mut self, ui: &mut egui::Ui) {
        ui.label(
            "FFmpeg is required to export. Download it to hanstat’s app data or install it on PATH.",
        );
        if let Some(download) = &self.download {
            let done = download.bytes.load(std::sync::atomic::Ordering::Relaxed);
            let total = windows::PACKAGE.total_size();
            let text = if done >= total {
                "Installing FFmpeg…".to_owned()
            } else {
                format!(
                    "Downloading FFmpeg · {:.0}%",
                    100.0 * done as f64 / total as f64
                )
            };
            ui.add(egui::ProgressBar::new(done as f32 / total as f32).text(text));
        } else {
            ui.horizontal(|ui| {
                let mib = windows::PACKAGE.total_size() as f64 / (1024.0 * 1024.0);
                if ui
                    .button(format!("Download FFmpeg ({mib:.0} MiB)"))
                    .clicked()
                {
                    self.error = None;
                    self.download = Some(windows::Download::start(ui.ctx()));
                }
                ui.hyperlink_to("FFmpeg downloads", DOWNLOAD_PAGE);
            });
        }
        if let Some(error) = &self.error {
            ui.colored_label(crate::palette::error(), error);
        }
    }
}

#[cfg(not(windows))]
fn installed_copy() -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn installed_copy() -> Option<PathBuf> {
    let dir = windows::directory();
    windows::PACKAGE
        .is_present_at(&dir)
        .then(|| dir.join("ffmpeg.exe"))
}

#[cfg(windows)]
mod windows {
    use super::Ffmpeg;
    use crate::reader::tts::local;
    use eframe::egui;
    use hanstat::models::{Member, Model, ModelFile, Unpack, app_dir};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::sync::mpsc::Receiver;

    // Versioned Gyan Essentials ZIP, listed by ffmpeg.org for Windows.
    // The pinned SHA-256 is checked before unpacking either member.
    pub(super) const PACKAGE: Model = Model {
        id: "ffmpeg-9.0.2",
        title: "FFmpeg 9.0.2 Essentials (Gyan)",
        licence: "GPL-3.0",
        homepage: "https://www.gyan.dev/ffmpeg/builds/",
        files: &[ModelFile {
            name: "ffmpeg-9.0.2-essentials_build.zip",
            url: "https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip",
            sha256: "60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba",
            size: 114_768_076,
            unpack: Unpack::Zip(&[
                Member {
                    path: "ffmpeg-9.0.2-essentials_build/LICENSE",
                    name: "LICENSE.txt",
                    size: 35_147,
                },
                Member {
                    path: "ffmpeg-9.0.2-essentials_build/bin/ffmpeg.exe",
                    name: "ffmpeg.exe",
                    size: 105_423_872,
                },
            ]),
        }],
    };

    pub(super) fn directory() -> PathBuf {
        app_dir().join(PACKAGE.id)
    }

    pub(super) struct Download {
        pub bytes: Arc<AtomicU64>,
        pub receiver: Receiver<Result<Option<Ffmpeg>, String>>,
    }

    impl Download {
        pub(super) fn start(ctx: &egui::Context) -> Self {
            let bytes = Arc::new(AtomicU64::new(0));
            let count = bytes.clone();
            let repaint = super::super::repaint(ctx);
            let receiver = local::spawn("audiobook-ffmpeg", repaint.clone(), move || {
                PACKAGE.ensure_at(&directory(), &|done, _| {
                    count.store(done, Relaxed);
                    repaint();
                })?;
                Ffmpeg::find()
            });
            Self { bytes, receiver }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_is_missing_when_no_candidate_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Ffmpeg::find_with(
                [dir.path().join("missing-system")],
                Some(dir.path().join("missing-managed")),
            )
            .unwrap()
            .is_none()
        );
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "downloads the pinned 109 MiB Windows FFmpeg ZIP"]
    fn verified_windows_download_is_used_when_path_has_no_encoder() {
        let dir = tempfile::tempdir().unwrap();
        windows::PACKAGE.ensure_at(dir.path(), &|_, _| {}).unwrap();
        assert!(windows::PACKAGE.is_present_at(dir.path()));
        let exe = dir.path().join("ffmpeg.exe");
        let ffmpeg = Ffmpeg::find_with([], Some(exe.clone())).unwrap().unwrap();
        assert_eq!(ffmpeg.0, exe);
        use crate::reader::tts::audiobook::output::{Format, check_ffmpeg};
        check_ffmpeg(&ffmpeg, Format::M4b).unwrap();
        check_ffmpeg(&ffmpeg, Format::Opus).unwrap();
    }
}
