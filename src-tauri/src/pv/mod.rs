//! Pairview modules. Everything Pairview adds to the backend lives under `pv`,
//! so upstream RapidRAW files only need small `// PV:` hooks (docs/IMPLEMENTATION_PLAN.md, A1).

pub mod assets;
pub mod catalog;
pub mod heif;
pub mod locations;
pub mod scan;
