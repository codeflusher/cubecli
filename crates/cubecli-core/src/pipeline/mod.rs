//! Install orchestration: downloading cores and running their install
//! recipes, generic over provider traits so it is testable with mocks.

pub mod addon_install;
pub mod modlist;
pub mod provision;

pub use addon_install::{
    AddonRequest, find_outdated, install_addon, parse_request, uninstall_addon,
};
pub use modlist::{
    ImportReport, ProjectExport, ScanReport, SourceLookup, export_from_project, export_from_scan,
    import_modlist,
};
pub use provision::{FileFetcher, ProvisionOptions, ProvisionReport};
