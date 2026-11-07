//! Pairview modules. Everything Pairview adds to the backend lives under `pv`,
//! so upstream RapidRAW files only need small `// PV:` hooks (docs/IMPLEMENTATION_PLAN.md, A1).

pub mod assets;
pub mod catalog;
pub mod commands;
pub mod edit;
pub mod exif_read;
pub mod heif;
pub mod history;
pub mod intent;
pub mod lifecycle;
pub mod locations;
pub mod look;
#[cfg(all(test, target_os = "macos"))]
mod look_job;
pub mod looks;
pub mod metadata;
mod nas_check;
pub mod queries;
pub mod scan;
pub mod signals;
pub mod slim;
pub mod thumbs;
pub mod triage;
pub mod viewer;
pub mod xmp;
