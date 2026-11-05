//! Pairview modules. Everything Pairview adds to the backend lives under `pv`,
//! so upstream RapidRAW files only need small `// PV:` hooks (docs/IMPLEMENTATION_PLAN.md, A1).

pub mod assets;
pub mod catalog;
pub mod commands;
pub mod exif_read;
pub mod heif;
pub mod intent;
pub mod lifecycle;
pub mod locations;
pub mod metadata;
mod nas_check;
pub mod queries;
pub mod scan;
pub mod signals;
pub mod thumbs;
pub mod viewer;
pub mod xmp;
