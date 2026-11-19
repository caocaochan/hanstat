//! Segmentation models are not bundled (their licences differ from this
//! project's). They are downloaded on first use from their official Hugging
//! Face repositories or GitHub releases, pinned to an exact commit or release
//! and verified by SHA-256, then cached in `models` in [`cache_dir`]
//! (`%LOCALAPPDATA%\hanstat\models`; override: `HANSTAT_MODEL_DIR`). SudachiDict is the exception: it follows
//! its newest GitHub release (see [`sudachi`]), verified by the SHA-256 that
//! GitHub reports for the asset.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    /// What to keep of the download.
    pub unpack: Unpack,
}

/// One file taken out of a downloaded archive.
pub struct Member {
    /// Path inside the archive (`/`-separated).
    pub path: &'static str,
    /// File name it is saved as in the model folder.
    pub name: &'static str,
    /// Expected size; 0 when not known in advance (the archive was verified
    /// by its SHA-256, and members are written under a temporary name).
    pub size: u64,
}

pub enum Unpack {
    /// Keep the download as is, named [`ModelFile::name`].
    Keep,
    /// Extract these members of a ZIP archive, then delete the archive.
    Zip(&'static [Member]),
    /// Extract these members of a gzip-compressed tar archive, then delete the archive.
    TarGz(&'static [Member]),
}

impl ModelFile {
    /// Everything this file provides is in `dir` with the expected size.
    fn is_present(&self, dir: &Path) -> bool {
        let ok = |name: &str, size: u64| {
            std::fs::metadata(dir.join(name)).is_ok_and(|m| size == 0 || m.len() == size)
        };
        match self.unpack {
            Unpack::Keep => ok(self.name, self.size),
            Unpack::Zip(ms) | Unpack::TarGz(ms) => ms.iter().all(|m| ok(m.name, m.size)),
        }
    }
}

pub struct Model {
    pub id: &'static str,
    pub title: &'static str,
    pub licence: &'static str,
    pub homepage: &'static str,
    pub files: &'static [ModelFile],
}

pub const LTP: Model = Model {
    id: "ltp-legacy",
    title: "LTP legacy CWS model (HIT-SCIR)",
    licence: "LTP licence: free for universities, research institutes and individual researchers; commercial use requires a paid licence from HIT-SCIR.",
    homepage: "https://huggingface.co/LTP/legacy",
    files: &[ModelFile {
        name: "cws_model.bin",
        url: "https://huggingface.co/LTP/legacy/resolve/aca6c995c6ca7ab2e4cba4af34964bcc37742fe9/cws_model.bin",
        sha256: "28fc2db628b27d4f69162d697d2294671784c7a61e0b4ce414e31060cfd20592",
        size: 31_977_765,
        unpack: Unpack::Keep,
    }],
};

macro_rules! ckip_url {
    ($f:literal) => {
        concat!("https://huggingface.co/ckiplab/bert-tiny-chinese-ws/resolve/f4a11d4b00b06502c260d8134a375a4000b09d7b/", $f)
    };
}

pub const CKIP: Model = Model {
    id: "ckip-bert-tiny-chinese-ws",
    title: "CKIP bert-tiny-chinese-ws (Academia Sinica CKIP Lab)",
    licence: "GPL-3.0",
    homepage: "https://huggingface.co/ckiplab/bert-tiny-chinese-ws",
    files: &[
        ModelFile {
            name: "config.json",
            url: ckip_url!("config.json"),
            sha256: "ce3c02e59fd3456a2d1cda726c3fbf86dfaa4bc544590aceaa0c29d5ac60d463",
            size: 861,
            unpack: Unpack::Keep,
        },
        ModelFile {
            name: "vocab.txt",
            url: ckip_url!("vocab.txt"),
            sha256: "45bbac6b341c319adc98a532532882e91a9cefc0329aa57bac9ae761c27b291c",
            size: 109_540,
            unpack: Unpack::Keep,
        },
        ModelFile {
            name: "pytorch_model.bin",
            url: ckip_url!("pytorch_model.bin"),
            sha256: "29d3ccfe939242b1691e8293d0cd30b5fd6b80333bf11967f22a6a7d5a213189",
            size: 45_802_231,
            unpack: Unpack::Keep,
        },
    ],
};

macro_rules! kiwi_url {
    ($f:literal) => {
        concat!(
            "https://github.com/bab2min/Kiwi/releases/download/v0.24.1/",
            $f
        )
    };
}

macro_rules! kiwi_model_files {
    ($($f:literal = $size:literal),* $(,)?) => {
        &[$(Member { path: concat!("models/cong/base/", $f), name: $f, size: $size }),*]
    };
}

/// Kiwi's CoNg base model, the same on every platform.
const KIWI_MODEL: ModelFile = ModelFile {
    name: "kiwi_model_v0.24.1_base.tgz",
    url: kiwi_url!("kiwi_model_v0.24.1_base.tgz"),
    sha256: "50d163e1af817444ef21f7ec5ea07ef69fb347282d57a6b49260f2039d4dc346",
    size: 87_967_233,
    unpack: Unpack::TarGz(kiwi_model_files![
        "combiningRule.txt" = 3_611,
        "cong.mdl" = 75_854_705,
        "default.dict" = 3_090_954,
        "dialect.dict" = 644,
        "extract.mdl" = 17_370,
        "multi.dict" = 12_064_440,
        "nounchr.mdl" = 9_734_234,
        "sj.morph" = 8_462_892,
        "typo.dict" = 395,
    ]),
};

/// Kiwi's C library: this platform's release build and the file taken out of
/// it. Only Windows x64, macOS arm64 and Linux x86_64/aarch64 are wired up;
/// elsewhere [`KIWI_LIB`] is `None` and Korean books can't be analysed.
#[cfg(all(windows, target_arch = "x86_64"))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = Some("kiwi.dll");
    pub const FILES: &[ModelFile] = &[
        ModelFile {
            name: "kiwi_win_x64_v0.24.1.zip",
            url: kiwi_url!("kiwi_win_x64_v0.24.1.zip"),
            sha256: "857576c5e8492ec66f8327e35572b20f286b092a8e760fe780ace1a99afc3b30",
            size: 38_261_195,
            unpack: Unpack::Zip(&[Member {
                path: "lib/kiwi.dll",
                name: "kiwi.dll",
                size: 23_307_264,
            }]),
        },
        KIWI_MODEL,
    ];
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = Some("libkiwi.dylib");
    pub const FILES: &[ModelFile] = &[
        ModelFile {
            name: "kiwi_mac_arm64_v0.24.1.tgz",
            url: kiwi_url!("kiwi_mac_arm64_v0.24.1.tgz"),
            sha256: "510680643980c4af94bee57151020b6373a098bac809084e7cb26080fc5bd9b9",
            size: 41_371_847,
            // libkiwi.dylib and libkiwi.0.dylib are symlinks to this file.
            unpack: Unpack::TarGz(&[Member {
                path: "lib/libkiwi.0.24.1.dylib",
                name: "libkiwi.dylib",
                size: 28_742_272,
            }]),
        },
        KIWI_MODEL,
    ];
}

/// Linux: Kiwi's manylinux2014 builds (glibc 2.17, libstdc++), which load on
/// any current distribution and in the Flatpak runtime.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = Some("libkiwi.so");
    pub const FILES: &[ModelFile] = &[
        ModelFile {
            name: "kiwi_lnx_x86_64_v0.24.1.tgz",
            url: kiwi_url!("kiwi_lnx_x86_64_v0.24.1.tgz"),
            sha256: "534f3b507784ea0f2c26884a13c22f2bfa573f86b6c20b9856feb9050a88f47a",
            size: 75_394_334,
            // libkiwi.so and libkiwi.so.0 are symlinks to this file.
            unpack: Unpack::TarGz(&[Member {
                path: "lib/libkiwi.so.0.24.1",
                name: "libkiwi.so",
                size: 58_594_768,
            }]),
        },
        KIWI_MODEL,
    ];
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = Some("libkiwi.so");
    pub const FILES: &[ModelFile] = &[
        ModelFile {
            name: "kiwi_lnx_aarch64_v0.24.1.tgz",
            url: kiwi_url!("kiwi_lnx_aarch64_v0.24.1.tgz"),
            sha256: "ff15838604039130d24e2803ed87c2edc36665f9ff24be8a23b257c2860bc25b",
            size: 34_868_989,
            unpack: Unpack::TarGz(&[Member {
                path: "lib/libkiwi.so.0.24.1",
                name: "libkiwi.so",
                size: 25_070_032,
            }]),
        },
        KIWI_MODEL,
    ];
}

#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
mod kiwi_lib {
    use super::*;
    pub const NAME: Option<&str> = None;
    pub const FILES: &[ModelFile] = &[KIWI_MODEL];
}

/// File name of Kiwi's C library in [`KIWI`]'s folder; `None` on platforms
/// without a build of it.
pub const KIWI_LIB: Option<&str> = kiwi_lib::NAME;

/// Kiwi Korean morphological analyser (library + CoNg base model). The
/// library is loaded at run time (see `crate::kiwi`), so a build never needs it.
pub const KIWI: Model = Model {
    id: "kiwi-v0.24.1",
    title: "Kiwi v0.24.1 Korean morphological analyser (bab2min)",
    licence: "Apache-2.0",
    homepage: "https://github.com/bab2min/Kiwi",
    files: kiwi_lib::FILES,
};

macro_rules! llama_url {
    ($f:literal) => {
        concat!(
            "https://github.com/ggml-org/llama.cpp/releases/download/b12464/",
            $f
        )
    };
}

/// llama.cpp's server from its b12464 release: the one program the writing-
/// style rating runs (see `crate::style`), with the libraries it loads. On
/// Windows and Linux the Vulkan build, which uses any GPU with a Vulkan
/// driver (NVIDIA, AMD, Intel) and falls back to the CPU, choosing the CPU
/// library built for the processor (`ggml-cpu-*`); on macOS the Metal build.
/// macOS and Linux load the libraries by their versioned names
/// (`@rpath/libllama.1.dylib`, `libllama.so.1`, the archive's symlinks), so
/// each is saved under that name. Only Windows x64, macOS arm64 and Linux
/// x86_64 are wired up; elsewhere [`LLAMA_SERVER`] is `None`.
#[cfg(all(windows, target_arch = "x86_64"))]
mod llama_bin {
    use super::*;
    macro_rules! m {
        ($($f:literal = $size:literal),* $(,)?) => {
            &[$(Member { path: $f, name: $f, size: $size }),*]
        };
    }
    pub const SERVER: Option<&str> = Some("llama-server.exe");
    pub const FILES: &[ModelFile] = &[ModelFile {
        name: "llama-b12464-bin-win-vulkan-x64.zip",
        url: llama_url!("llama-b12464-bin-win-vulkan-x64.zip"),
        sha256: "839455639ced0f6ad50e971d37c89050e0e8b952c12e1c62a78823940fb93073",
        size: 38_354_249,
        unpack: Unpack::Zip(m![
            "llama-server.exe" = 9_728,
            "llama-server-impl.dll" = 10_356_224,
            "llama-common.dll" = 8_592_384,
            "llama.dll" = 3_910_144,
            "mtmd.dll" = 2_576_384,
            "ggml.dll" = 78_848,
            "ggml-base.dll" = 832_000,
            "ggml-vulkan.dll" = 42_398_720,
            "ggml-rpc.dll" = 234_496,
            "libomp.dll" = 768_000,
            "ggml-cpu-x64.dll" = 932_352,
            "ggml-cpu-sse42.dll" = 939_520,
            "ggml-cpu-sandybridge.dll" = 1_316_352,
            "ggml-cpu-ivybridge.dll" = 1_348_096,
            "ggml-cpu-piledriver.dll" = 1_341_440,
            "ggml-cpu-haswell.dll" = 1_466_880,
            "ggml-cpu-skylakex.dll" = 1_688_064,
            "ggml-cpu-cannonlake.dll" = 1_695_232,
            "ggml-cpu-cascadelake.dll" = 1_668_096,
            "ggml-cpu-icelake.dll" = 1_674_752,
            "ggml-cpu-cooperlake.dll" = 1_677_824,
            "ggml-cpu-alderlake.dll" = 1_461_760,
            "ggml-cpu-zen4.dll" = 1_683_968,
            "ggml-cpu-sapphirerapids.dll" = 1_954_304,
            "LICENSE-LLVM-OpenMP" = 19_741,
        ]),
    }];
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod llama_bin {
    use super::*;
    macro_rules! m {
        ($($f:literal => $name:literal = $size:literal),* $(,)?) => {
            &[$(Member { path: concat!("llama-b12464/", $f), name: $name, size: $size }),*]
        };
    }
    pub const SERVER: Option<&str> = Some("llama-server");
    pub const FILES: &[ModelFile] = &[ModelFile {
        name: "llama-b12464-bin-macos-arm64.tar.gz",
        url: llama_url!("llama-b12464-bin-macos-arm64.tar.gz"),
        sha256: "95cb3cf68c5534bebe598c4f95576fcca28444627de2373293512759e3ffa925",
        size: 17_065_398,
        unpack: Unpack::TarGz(m![
            "llama-server" => "llama-server" = 49_984,
            "libllama-server-impl.dylib" => "libllama-server-impl.dylib" = 10_032_264,
            "libllama-common.1.0.0.dylib" => "libllama-common.1.dylib" = 8_224_456,
            "libllama.1.0.0.dylib" => "libllama.1.dylib" = 3_852_064,
            "libmtmd.1.0.0.dylib" => "libmtmd.1.dylib" = 1_948_896,
            "libggml.1.0.0.dylib" => "libggml.1.dylib" = 60_016,
            "libggml-base.1.0.0.dylib" => "libggml-base.1.dylib" = 770_760,
            "libggml-cpu.1.0.0.dylib" => "libggml-cpu.1.dylib" = 959_168,
            "libggml-metal.1.0.0.dylib" => "libggml-metal.1.dylib" = 5_688_296,
            "libggml-blas.1.0.0.dylib" => "libggml-blas.1.dylib" = 58_776,
            "libggml-rpc.1.0.0.dylib" => "libggml-rpc.1.dylib" = 243_488,
            "LICENSE" => "LICENSE" = 1_078,
        ]),
    }];
}

/// Needs the system's `libgomp.so.1` (GCC's OpenMP runtime) and, for the
/// GPU, `libvulkan.so.1`; without a Vulkan driver it runs on the CPU.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod llama_bin {
    use super::*;
    macro_rules! m {
        ($($f:literal => $name:literal = $size:literal),* $(,)?) => {
            &[$(Member { path: concat!("llama-b12464/", $f), name: $name, size: $size }),*]
        };
    }
    pub const SERVER: Option<&str> = Some("llama-server");
    pub const FILES: &[ModelFile] = &[ModelFile {
        name: "llama-b12464-bin-ubuntu-vulkan-x64.tar.gz",
        url: llama_url!("llama-b12464-bin-ubuntu-vulkan-x64.tar.gz"),
        sha256: "78fb7eefb8f022e0f181083e7f868917efc184b38cd9282ffc450c950bbae018",
        size: 36_478_662,
        unpack: Unpack::TarGz(m![
            "llama-server" => "llama-server" = 30_840,
            "libllama-server-impl.so" => "libllama-server-impl.so" = 8_519_704,
            "libllama-common.so.1.0.0" => "libllama-common.so.1" = 7_000_480,
            "libllama.so.1.0.0" => "libllama.so.1" = 5_540_136,
            "libmtmd.so.1.0.0" => "libmtmd.so.1" = 2_671_864,
            "libggml.so.1.0.0" => "libggml.so.1" = 51_040,
            "libggml-base.so.1.0.0" => "libggml-base.so.1" = 937_456,
            "libggml-vulkan.so" => "libggml-vulkan.so" = 41_594_816,
            "libggml-rpc.so" => "libggml-rpc.so" = 328_712,
            "libggml-cpu-x64.so" => "libggml-cpu-x64.so" = 976_392,
            "libggml-cpu-sse42.so" => "libggml-cpu-sse42.so" = 976_272,
            "libggml-cpu-sandybridge.so" => "libggml-cpu-sandybridge.so" = 1_364_536,
            "libggml-cpu-ivybridge.so" => "libggml-cpu-ivybridge.so" = 1_399_368,
            "libggml-cpu-piledriver.so" => "libggml-cpu-piledriver.so" = 1_395_272,
            "libggml-cpu-haswell.so" => "libggml-cpu-haswell.so" = 1_459_656,
            "libggml-cpu-skylakex.so" => "libggml-cpu-skylakex.so" = 1_618_576,
            "libggml-cpu-cannonlake.so" => "libggml-cpu-cannonlake.so" = 1_618_576,
            "libggml-cpu-cascadelake.so" => "libggml-cpu-cascadelake.so" = 1_634_664,
            "libggml-cpu-icelake.so" => "libggml-cpu-icelake.so" = 1_634_664,
            "libggml-cpu-cooperlake.so" => "libggml-cpu-cooperlake.so" = 1_634_736,
            "libggml-cpu-alderlake.so" => "libggml-cpu-alderlake.so" = 1_455_560,
            "libggml-cpu-zen4.so" => "libggml-cpu-zen4.so" = 1_634_736,
            "libggml-cpu-sapphirerapids.so" => "libggml-cpu-sapphirerapids.so" = 1_901_032,
            "LICENSE" => "LICENSE" = 1_078,
        ]),
    }];
}

#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
)))]
mod llama_bin {
    use super::*;
    pub const SERVER: Option<&str> = None;
    pub const FILES: &[ModelFile] = &[];
}

/// File name of llama.cpp's server in [`LLAMA_CPP`]'s folder; `None` on
/// platforms without a build of it.
pub const LLAMA_SERVER: Option<&str> = llama_bin::SERVER;

/// llama.cpp's server, which runs [`STYLE_LLM`] for the writing-style rating.
pub const LLAMA_CPP: Model = Model {
    id: "llama.cpp-b12464",
    title: "llama.cpp b12464 server (ggml-org)",
    licence: "MIT (LLVM OpenMP on Windows: Apache-2.0 with LLVM exception)",
    homepage: "https://github.com/ggml-org/llama.cpp",
    files: llama_bin::FILES,
};

/// The language model that rates writing style: Qwen3.5-4B, 4-bit (Q4_K_M)
/// GGUF as converted by Unsloth (Qwen publishes no GGUF of it), pinned to a
/// commit of that repository.
pub const STYLE_LLM: Model = Model {
    id: "qwen3.5-4b-q4km",
    title: "Qwen3.5-4B, Q4_K_M (Alibaba Qwen; GGUF by Unsloth)",
    licence: "Apache-2.0",
    homepage: "https://huggingface.co/Qwen/Qwen3.5-4B",
    files: &[ModelFile {
        name: "Qwen3.5-4B-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3.5-4B-GGUF/resolve/e87f176479d0855a907a41277aca2f8ee7a09523/Qwen3.5-4B-Q4_K_M.gguf",
        sha256: "00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4",
        size: 2_740_937_888,
        unpack: Unpack::Keep,
    }],
};

macro_rules! qwentts_url {
    ($f:literal) => {
        concat!(
            "https://github.com/caocaochan/hanstat/releases/download/qwentts.cpp-2edb35f/",
            $f
        )
    };
}

/// qwentts.cpp's `tts-server` at commit 2edb35f, which runs
/// [`QWEN_TTS_MODEL`] for the reader's local read-aloud voice. Upstream
/// publishes no binaries: hanstat's `.github/workflows/qwentts.yml` builds
/// them (one static executable each: Vulkan on Windows and Linux, Metal on
/// macOS) into a prerelease of this repository. Downloaded only when the
/// user asks for the local voice.
#[cfg(all(windows, target_arch = "x86_64"))]
mod qwentts_bin {
    use super::*;
    pub const SERVER: Option<&str> = Some("tts-server.exe");
    pub const FILES: &[ModelFile] = &[ModelFile {
        name: "qwentts-2edb35f-windows-x64-vulkan.zip",
        url: qwentts_url!("qwentts-2edb35f-windows-x64-vulkan.zip"),
        sha256: "bf9e9785bb8690c00e2426a69926293773e9aab4571d01bd7d603d1beb57cda9",
        size: 14_012_224,
        unpack: Unpack::Zip(&[
            Member {
                path: "tts-server.exe",
                name: "tts-server.exe",
                size: 44_676_608,
            },
            Member {
                path: "LICENSE-qwentts.cpp.txt",
                name: "LICENSE-qwentts.cpp.txt",
                size: 1_087,
            },
            Member {
                path: "LICENSE-ggml.txt",
                name: "LICENSE-ggml.txt",
                size: 1_099,
            },
        ]),
    }];
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod qwentts_bin {
    use super::*;
    pub const SERVER: Option<&str> = Some("tts-server");
    pub const FILES: &[ModelFile] = &[ModelFile {
        name: "qwentts-2edb35f-linux-x86_64-vulkan.tar.gz",
        url: qwentts_url!("qwentts-2edb35f-linux-x86_64-vulkan.tar.gz"),
        sha256: "9b6b4924f94ba46a447319f2a702a5c17de507dc899c51add3576779daeb2a39",
        size: 14_547_505,
        unpack: Unpack::TarGz(&[
            Member {
                path: "tts-server",
                name: "tts-server",
                size: 45_806_272,
            },
            Member {
                path: "LICENSE-qwentts.cpp.txt",
                name: "LICENSE-qwentts.cpp.txt",
                size: 1_087,
            },
            Member {
                path: "LICENSE-ggml.txt",
                name: "LICENSE-ggml.txt",
                size: 1_078,
            },
        ]),
    }];
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod qwentts_bin {
    use super::*;
    pub const SERVER: Option<&str> = Some("tts-server");
    pub const FILES: &[ModelFile] = &[ModelFile {
        name: "qwentts-2edb35f-macos-arm64-metal.tar.gz",
        url: qwentts_url!("qwentts-2edb35f-macos-arm64-metal.tar.gz"),
        sha256: "e6ca70ba1a42e2e402d255bc4ad2394291e438d8233717041378380bb19df80e",
        size: 2_174_181,
        unpack: Unpack::TarGz(&[
            Member {
                path: "tts-server",
                name: "tts-server",
                size: 8_213_384,
            },
            Member {
                path: "LICENSE-qwentts.cpp.txt",
                name: "LICENSE-qwentts.cpp.txt",
                size: 1_087,
            },
            Member {
                path: "LICENSE-ggml.txt",
                name: "LICENSE-ggml.txt",
                size: 1_078,
            },
        ]),
    }];
}

#[cfg(not(any(
    all(windows, target_arch = "x86_64"),
    all(target_os = "macos", target_arch = "aarch64"),
    all(target_os = "linux", target_arch = "x86_64")
)))]
mod qwentts_bin {
    use super::*;
    pub const SERVER: Option<&str> = None;
    pub const FILES: &[ModelFile] = &[];
}

/// File name of qwentts.cpp's server in [`QWEN_TTS`]'s folder; `None` on
/// platforms without a build of it.
pub const QWEN_TTS_SERVER: Option<&str> = qwentts_bin::SERVER;

pub const QWEN_TTS: Model = Model {
    id: "qwentts.cpp-2edb35f",
    title: "qwentts.cpp 2edb35f tts-server (ServeurpersoCom)",
    licence: "MIT",
    homepage: "https://github.com/ServeurpersoCom/qwentts.cpp",
    files: qwentts_bin::FILES,
};

const QWEN_TTS_CODEC_FILE: ModelFile = ModelFile {
    name: "qwen-tokenizer-12hz-Q8_0.gguf",
    url: "https://huggingface.co/Serveurperso/Qwen3-TTS-GGUF/resolve/b7ee2e8c7459c3bea99da23e3d178125a7d1713c/qwen-tokenizer-12hz-Q8_0.gguf",
    sha256: "1883beeed99348fc35e23dd225e9082f93f6f8c109330a33d935baa8acdbfd94",
    size: 291_150_624,
    unpack: Unpack::Keep,
};

/// The local read-aloud voice: Qwen3-TTS 12Hz 1.7B CustomVoice (nine named
/// speakers, style instructions) and its audio tokenizer, both Q8_0 GGUF as
/// converted by qwentts.cpp's author, pinned to a commit. Q8_0 over BF16:
/// on an RTX 5070 (Vulkan) it ran twice as fast (6.5× vs 3.3× real time)
/// in 3.7 instead of 6.6 GB, with no difference the user preferred.
pub const QWEN_TTS_MODEL: Model = Model {
    id: "qwen3-tts-1.7b-customvoice-q8_0",
    title: "Qwen3-TTS 12Hz 1.7B CustomVoice, Q8_0 (Alibaba Qwen; GGUF by Serveurperso)",
    licence: "Apache-2.0",
    homepage: "https://huggingface.co/Qwen/Qwen3-TTS-12Hz-1.7B-CustomVoice",
    files: &[
        ModelFile {
            name: "qwen-talker-1.7b-customvoice-Q8_0.gguf",
            url: "https://huggingface.co/Serveurperso/Qwen3-TTS-GGUF/resolve/b7ee2e8c7459c3bea99da23e3d178125a7d1713c/qwen-talker-1.7b-customvoice-Q8_0.gguf",
            sha256: "cab2cff67a0a557310febe558dc83076b28ed790e491867eb2751759f4cd89fa",
            size: 2_042_834_304,
            unpack: Unpack::Keep,
        },
        QWEN_TTS_CODEC_FILE,
    ],
};

/// The same tokenizer and cache path, for voices that don't need CustomVoice.
pub const QWEN_TTS_CODEC: Model = Model {
    files: &[QWEN_TTS_CODEC_FILE],
    ..QWEN_TTS_MODEL
};

/// VoiceDesign makes a sample once; Base uses that reference to read books.
pub const QWEN_TTS_DESIGN: Model = Model {
    id: "qwen3-tts-1.7b-design-base-q8_0",
    title: "Qwen3-TTS 12Hz 1.7B VoiceDesign and Base, Q8_0 (Alibaba Qwen; GGUF by Serveurperso)",
    licence: "Apache-2.0",
    homepage: "https://huggingface.co/Serveurperso/Qwen3-TTS-GGUF",
    files: &[
        ModelFile {
            name: "qwen-talker-1.7b-voicedesign-Q8_0.gguf",
            url: "https://huggingface.co/Serveurperso/Qwen3-TTS-GGUF/resolve/b7ee2e8c7459c3bea99da23e3d178125a7d1713c/qwen-talker-1.7b-voicedesign-Q8_0.gguf",
            sha256: "575610ab1ddcca4dca6bd9a64bcd859d93bbad8764f9cab24e1dbc0c51f62276",
            size: 2_042_833_824,
            unpack: Unpack::Keep,
        },
        ModelFile {
            name: "qwen-talker-1.7b-base-Q8_0.gguf",
            url: "https://huggingface.co/Serveurperso/Qwen3-TTS-GGUF/resolve/b7ee2e8c7459c3bea99da23e3d178125a7d1713c/qwen-talker-1.7b-base-Q8_0.gguf",
            sha256: "4b9a33a236908dd9435a42f7a396e38038329d053b704342a6413c08544c4fda",
            size: 2_079_448_256,
            unpack: Unpack::Keep,
        },
    ],
};

/// SudachiDict full 20261015 from its GitHub release, the dictionary of the
/// Sudachi Japanese analyser (see `crate::japanese`). Full rather than core:
/// it adds proper nouns and set phrases, so more names and nicknames stay
/// whole (なっちゃん, 持崎湯葉). The version hanstat was tested with, and the fallback when the newest release can't be found or
/// doesn't load (see [`sudachi`]). GitHub is used rather than Works
/// Applications' S3 bucket, which serves the same file far more slowly
/// outside Japan.
pub const SUDACHI: Model = Model {
    id: "sudachidict-full-20261015",
    title: "SudachiDict full 20261015 (Works Applications)",
    licence: "Apache-2.0",
    homepage: "https://github.com/WorksApplications/SudachiDict",
    files: &[ModelFile {
        name: "sudachi-dictionary-20261015-full.zip",
        url: "https://github.com/WorksApplications/SudachiDict/releases/download/v20261015/sudachi-dictionary-20261015-full.zip",
        sha256: "b38318ca26a444cd713a5a377a353308ef38c733c64cc0be905122527516c793",
        size: 137_444_817,
        unpack: Unpack::Zip(&[
            Member {
                path: "sudachi-dictionary-20261015/system_full.dic",
                name: "system_full.dic",
                size: 330_506_339,
            },
            Member {
                path: "sudachi-dictionary-20261015/LICENSE-2.0.txt",
                name: "LICENSE-2.0.txt",
                size: 11_358,
            },
            Member {
                path: "sudachi-dictionary-20261015/LEGAL",
                name: "LEGAL",
                size: 6_037,
            },
        ]),
    }],
};

const SUDACHI_REPO: &str = "WorksApplications/SudachiDict";

/// How often to ask GitHub for a newer SudachiDict.
const SUDACHI_CHECK_SECS: u64 = 24 * 60 * 60;

/// The last answer from GitHub, kept in the model cache so that it is asked
/// at most once a day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SudachiRelease {
    /// Unix time of the check.
    checked: u64,
    /// `20261015`.
    version: String,
    url: String,
    sha256: String,
    size: u64,
}

/// SudachiDict full from its newest GitHub release that has a full zip
/// (some releases have only Python wheels). GitHub is asked at most once a
/// day; without an answer, the newest dictionary already downloaded is used,
/// and failing that the tested [`SUDACHI`]. Resolved once per run.
pub fn sudachi() -> &'static Model {
    static M: OnceLock<&'static Model> = OnceLock::new();
    M.get_or_init(|| {
        let state = cache_root().join("sudachidict-latest.json");
        let saved: Option<SudachiRelease> = std::fs::read(&state)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let fresh = saved
            .as_ref()
            .filter(|s| now.saturating_sub(s.checked) < SUDACHI_CHECK_SECS)
            .cloned();
        let release = fresh.or_else(|| match latest_sudachi_release(now) {
            Ok(r) => {
                let _ = std::fs::create_dir_all(cache_root());
                let _ = std::fs::write(&state, serde_json::to_vec_pretty(&r).unwrap());
                Some(r)
            }
            Err(_) => saved,
        });
        match release {
            Some(r) if r.version == "20261015" => &SUDACHI,
            Some(r) => leak_sudachi_model(&r),
            None => sudachi_local(),
        }
    })
}

/// The newest SudachiDict already downloaded, or [`SUDACHI`]; never asks
/// the network (for display and quick checks).
pub fn sudachi_local() -> &'static Model {
    static M: OnceLock<&'static Model> = OnceLock::new();
    M.get_or_init(|| {
        let newest = std::fs::read_dir(cache_root())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let v = name.strip_prefix("sudachidict-full-")?.to_owned();
                (v.len() == 8
                    && v.bytes().all(|b| b.is_ascii_digit())
                    && e.path().join("system_full.dic").is_file())
                .then_some(v)
            })
            .max();
        match newest {
            Some(v) if v.as_str() > "20261015" => leak_sudachi_model(&SudachiRelease {
                checked: 0,
                url: sudachi_url(&v),
                version: v,
                // Already unpacked, so never downloaded again.
                sha256: String::new(),
                size: 0,
            }),
            _ => &SUDACHI,
        }
    })
}

fn sudachi_url(version: &str) -> String {
    format!(
        "https://github.com/{SUDACHI_REPO}/releases/download/v{version}/sudachi-dictionary-{version}-full.zip"
    )
}

#[derive(Deserialize)]
struct GhRelease {
    draft: bool,
    prerelease: bool,
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    /// `sha256:<hex>`, computed by GitHub.
    digest: Option<String>,
}

/// The newest release with a full zip and a SHA-256, from the list GitHub
/// returns newest first.
fn parse_sudachi_releases(json: &str, now: u64) -> Result<SudachiRelease, String> {
    let rs: Vec<GhRelease> =
        serde_json::from_str(json).map_err(|e| format!("Unexpected GitHub response: {e}"))?;
    let prefix = format!("https://github.com/{SUDACHI_REPO}/releases/download/");
    rs.iter()
        .filter(|r| !r.draft && !r.prerelease)
        .flat_map(|r| &r.assets)
        .find_map(|a| {
            let v = a
                .name
                .strip_prefix("sudachi-dictionary-")?
                .strip_suffix("-full.zip")?;
            let sha = a.digest.as_deref()?.strip_prefix("sha256:")?;
            (v.len() == 8
                && v.bytes().all(|b| b.is_ascii_digit())
                && sha.len() == 64
                && a.browser_download_url.starts_with(&prefix))
            .then(|| SudachiRelease {
                checked: now,
                version: v.to_owned(),
                url: a.browser_download_url.clone(),
                sha256: sha.to_ascii_lowercase(),
                size: a.size,
            })
        })
        .ok_or_else(|| "No SudachiDict release with a full dictionary zip".to_owned())
}

fn latest_sudachi_release(now: u64) -> Result<SudachiRelease, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .into();
    let json = agent
        .get(format!(
            "https://api.github.com/repos/{SUDACHI_REPO}/releases?per_page=10"
        ))
        .header("User-Agent", concat!("hanstat/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_sudachi_releases(&json, now)
}

/// A [`Model`] for a SudachiDict release found at run time (kept for the
/// rest of the run).
fn leak_sudachi_model(r: &SudachiRelease) -> &'static Model {
    let s = |x: String| -> &'static str { Box::leak(x.into_boxed_str()) };
    let v = &r.version;
    let members: &'static [Member] = Box::leak(Box::new([
        Member {
            path: s(format!("sudachi-dictionary-{v}/system_full.dic")),
            name: "system_full.dic",
            size: 0,
        },
        Member {
            path: s(format!("sudachi-dictionary-{v}/LICENSE-2.0.txt")),
            name: "LICENSE-2.0.txt",
            size: 0,
        },
        Member {
            path: s(format!("sudachi-dictionary-{v}/LEGAL")),
            name: "LEGAL",
            size: 0,
        },
    ]));
    let files: &'static [ModelFile] = Box::leak(Box::new([ModelFile {
        name: s(format!("sudachi-dictionary-{v}-full.zip")),
        url: s(r.url.clone()),
        sha256: s(r.sha256.clone()),
        size: r.size,
        unpack: Unpack::Zip(members),
    }]));
    Box::leak(Box::new(Model {
        id: s(format!("sudachidict-full-{v}")),
        title: s(format!("SudachiDict full {v} (Works Applications)")),
        licence: "Apache-2.0",
        homepage: "https://github.com/WorksApplications/SudachiDict",
        files,
    }))
}

/// hanstat's per-user folder for saved settings and the Library
/// (`%LOCALAPPDATA%\hanstat`, on macOS `~/Library/Application Support/hanstat`,
/// on Linux `$XDG_DATA_HOME/hanstat`, normally `~/.local/share/hanstat`; in
/// the Flatpak `~/.var/app/io.github.caocaochan.hanstat/data/hanstat`).
pub fn app_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        xdg_dir("XDG_DATA_HOME", ".local/share")
    };
    base.unwrap_or_else(std::env::temp_dir).join("hanstat")
}

/// hanstat's per-user folder for what can be downloaded or made again: the
/// models and the saved analyses. [`app_dir`] on Windows and macOS; on Linux
/// `$XDG_CACHE_HOME/hanstat`, normally `~/.cache/hanstat`.
pub fn cache_dir() -> PathBuf {
    if cfg!(any(windows, target_os = "macos")) {
        return app_dir();
    }
    xdg_dir("XDG_CACHE_HOME", ".cache")
        .unwrap_or_else(std::env::temp_dir)
        .join("hanstat")
}

/// An XDG base directory: `var` if it holds an absolute path (the spec says
/// to ignore a relative one), else `~/<fallback>`.
pub(crate) fn xdg_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
}

/// Root cache directory for all models.
pub fn cache_root() -> PathBuf {
    if let Some(d) = std::env::var_os("HANSTAT_MODEL_DIR") {
        return PathBuf::from(d);
    }
    cache_dir().join("models")
}

impl Model {
    pub fn dir(&self) -> PathBuf {
        cache_root().join(self.id)
    }

    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// True if every file is present with the expected size (hash checked on download).
    pub fn is_cached(&self) -> bool {
        let d = self.dir();
        self.files.iter().all(|f| f.is_present(&d))
    }

    /// Download any missing files. `progress(done_bytes, total_bytes)`.
    pub fn ensure(&self, progress: &dyn Fn(u64, u64)) -> Result<PathBuf, String> {
        let dir = self.dir();
        if self.is_cached() {
            return Ok(dir);
        }
        // One download at a time per process; re-check after waiting, since
        // another thread may have just finished the same files.
        let _guard = DOWNLOAD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if self.is_cached() {
            return Ok(dir);
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let total = self.total_size();
        let mut done_before = 0u64;
        for f in self.files {
            if f.is_present(&dir) {
                done_before += f.size;
                continue;
            }
            let dest = dir.join(f.name);
            // An archive left by an interrupted unpack was verified before it was renamed.
            if !std::fs::metadata(&dest).is_ok_and(|m| m.len() == f.size) {
                download(f, &dest, &|d| progress(done_before + d, total)).map_err(|e| {
                    format!(
                        "Downloading {} for {} failed: {e}\nURL: {}",
                        f.name, self.title, f.url
                    )
                })?;
            }
            match f.unpack {
                Unpack::Keep => {}
                Unpack::Zip(ms) => {
                    unpack_zip(&dest, &dir, ms)
                        .map_err(|e| format!("Unpacking {} failed: {e}", f.name))?;
                    let _ = std::fs::remove_file(&dest);
                }
                Unpack::TarGz(ms) => {
                    unpack_tar_gz(&dest, &dir, ms)
                        .map_err(|e| format!("Unpacking {} failed: {e}", f.name))?;
                    let _ = std::fs::remove_file(&dest);
                }
            }
            done_before += f.size;
        }
        progress(total, total);
        Ok(dir)
    }
}

/// Write one archive member to `dir/m.name` (via a temporary name), checking its size.
fn save_member(mut r: impl Read, dir: &Path, m: &Member) -> Result<(), String> {
    let dest = dir.join(m.name);
    let tmp = dest.with_extension(format!("{}.part", std::process::id()));
    let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let n = std::io::copy(&mut r, &mut out).map_err(|e| e.to_string())?;
    drop(out);
    if m.size != 0 && n != m.size {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: expected {} bytes, got {n}", m.path, m.size));
    }
    std::fs::rename(&tmp, &dest).or_else(|e| {
        let _ = std::fs::remove_file(&tmp);
        // Another process unpacked the same file first.
        if std::fs::metadata(&dest).is_ok_and(|x| m.size == 0 || x.len() == m.size) {
            Ok(())
        } else {
            Err(e.to_string())
        }
    })
}

fn unpack_zip(archive: &Path, dir: &Path, members: &[Member]) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| e.to_string())?;
    for m in members {
        let entry = z.by_name(m.path).map_err(|e| format!("{}: {e}", m.path))?;
        save_member(entry, dir, m)?;
    }
    Ok(())
}

fn unpack_tar_gz(archive: &Path, dir: &Path, members: &[Member]) -> Result<(), String> {
    let f = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::io::BufReader::new(f)));
    let mut found = vec![false; members.len()];
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?;
        let path = path.to_string_lossy().replace('\\', "/");
        let path = path.trim_start_matches("./");
        if let Some(i) = members.iter().position(|m| m.path == path) {
            save_member(entry, dir, &members[i])?;
            found[i] = true;
        }
    }
    match members.iter().zip(&found).find(|(_, f)| !**f) {
        Some((m, _)) => Err(format!("{} is missing from the archive", m.path)),
        None => Ok(()),
    }
}

static DOWNLOAD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn download(f: &ModelFile, dest: &std::path::Path, progress: &dyn Fn(u64)) -> Result<(), String> {
    if f.sha256.is_empty() {
        return Err("no checksum known for this file".to_owned());
    }
    let mut resp = ureq::get(f.url)
        .header("User-Agent", concat!("hanstat/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| e.to_string())?;
    // Per-process temp name so concurrent processes never share a partial file.
    let tmp = dest.with_extension(format!("{}.part", std::process::id()));
    let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().as_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut n_total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        n_total += n as u64;
        progress(n_total);
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    let got: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if got != f.sha256 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "checksum mismatch (expected {}, got {got}, {n_total} bytes)",
            f.sha256
        ));
    }
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        // Another process finished the same verified file first: that's fine.
        Err(_) if std::fs::metadata(dest).is_ok_and(|m| m.len() == f.size) => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_sudachi_release_with_a_full_zip() {
        let sha = "b38318ca26a444cd713a5a377a353308ef38c733c64cc0be905122527516c793";
        let url = |t: &str, n: &str| {
            format!("https://github.com/WorksApplications/SudachiDict/releases/download/{t}/{n}")
        };
        // Newest first; the newest has only wheels, a prerelease is skipped.
        let json = serde_json::json!([
            {"draft": false, "prerelease": true, "assets": [
                {"name": "sudachi-dictionary-20270101-full.zip", "size": 1,
                 "browser_download_url": url("v20270101", "sudachi-dictionary-20270101-full.zip"),
                 "digest": format!("sha256:{sha}")}]},
            {"draft": false, "prerelease": false, "assets": [
                {"name": "sudachidict_core-20261201-py3-none-any.whl", "size": 1,
                 "browser_download_url": url("v20261201", "sudachidict_core-20261201-py3-none-any.whl"),
                 "digest": format!("sha256:{sha}")}]},
            {"draft": false, "prerelease": false, "assets": [
                {"name": "sudachi-dictionary-20261015-core.zip", "size": 2,
                 "browser_download_url": url("v20261015", "sudachi-dictionary-20261015-core.zip"),
                 "digest": format!("sha256:{sha}")},
                {"name": "sudachi-dictionary-20261015-full.zip", "size": 137444817,
                 "browser_download_url": url("v20261015", "sudachi-dictionary-20261015-full.zip"),
                 "digest": format!("sha256:{}", sha.to_uppercase())}]}
        ])
        .to_string();
        let r = parse_sudachi_releases(&json, 7).unwrap();
        assert_eq!(r.version, "20261015");
        assert_eq!(r.size, 137_444_817);
        assert_eq!(r.sha256, sha);
        assert_eq!(r.checked, 7);
        assert_eq!(r.url, sudachi_url("20261015"));
        // An asset hosted elsewhere, or without a digest, is not used.
        let bad = json.replace(
            "https://github.com/WorksApplications/SudachiDict/",
            "https://example.com/",
        );
        assert!(parse_sudachi_releases(&bad, 0).is_err());
    }

    #[test]
    fn runtime_sudachi_model() {
        let m = leak_sudachi_model(&SudachiRelease {
            checked: 0,
            version: "20270101".into(),
            url: sudachi_url("20270101"),
            sha256: "ab".repeat(32),
            size: 9,
        });
        assert_eq!(m.id, "sudachidict-full-20270101");
        assert_eq!(m.files[0].name, "sudachi-dictionary-20270101-full.zip");
        let Unpack::Zip(ms) = m.files[0].unpack else {
            panic!()
        };
        assert_eq!(ms[0].path, "sudachi-dictionary-20270101/system_full.dic");
        assert_eq!(ms[0].name, "system_full.dic");
    }
}
