use std::fs;
use std::path::Path;

use crate::error::RociError;

pub(crate) fn remove_path(path: &Path) -> Result<(), RociError> {
    if !path.exists() {
        return Ok(());
    }

    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub(crate) fn copy_directory_recursive(source: &Path, destination: &Path) -> Result<(), RociError> {
    let metadata = fs::metadata(source)?;
    if !metadata.is_dir() {
        return Err(RociError::InvalidArgument(format!(
            "Skill source '{}' is not a directory",
            source.display()
        )));
    }

    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = destination.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            copy_directory_recursive(&source_path, &target_path)?;
            continue;
        }

        if file_type.is_symlink() {
            let linked_metadata = fs::metadata(&source_path)?;
            if linked_metadata.is_dir() {
                copy_directory_recursive(&source_path, &target_path)?;
            } else {
                fs::copy(&source_path, &target_path)?;
            }
            continue;
        }

        fs::copy(&source_path, &target_path)?;
    }

    Ok(())
}
