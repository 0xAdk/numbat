use std::path::Path;

use numbat::{
    resolver::{CodeSource, FileSystemImporter},
    Context,
};

pub fn get_test_context_without_prelude() -> Context {
    let module_path = Path::new("../modules");

    let mut importer = FileSystemImporter::default();
    importer.add_path(module_path);

    Context::new(importer)
}

pub fn get_test_context() -> Context {
    let mut context = get_test_context_without_prelude();

    assert!(context
        .interpret("use prelude", CodeSource::Text)
        .expect("Error while running prelude")
        .1
        .is_success());
    context
}
