/*
 * Licensed to the Apache Software Foundation (ASF) under one
 * or more contributor license agreements.  See the NOTICE file
 * distributed with this work for additional information
 * regarding copyright ownership.  The ASF licenses this file
 * to you under the Apache License, Version 2.0 (the
 * "License"); you may not use this file except in compliance
 * with the License.  You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

use maka_fs_tools::workspace::directory::PublishedDirectory;
use std::fs;

#[test]
fn published_directory_preserves_root_identity_containment_and_scan_budget() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("published");
    fs::create_dir_all(path.join("inside")).unwrap();
    fs::write(path.join("plain-file"), "not a directory").unwrap();
    let root = PublishedDirectory::open(&path).unwrap();
    assert_eq!(root.directory_names(&[], 2).unwrap(), ["inside"]);
    assert!(
        root.directory_names(&[], 1).is_err(),
        "non-directory entries also consume the scan budget"
    );
    assert_eq!(
        root.resolve(&["inside".into()]).unwrap(),
        path.join("inside").canonicalize().unwrap()
    );
    for segment in [".", "..", "", "inside/..", "inside\\..", "\0"] {
        assert!(root.resolve(&[segment.into()]).is_err(), "{segment:?}");
    }
    #[cfg(unix)]
    {
        let outside = temp.path().join("private");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, path.join("escape")).unwrap();
        std::os::unix::fs::symlink(path.join("inside"), path.join("alias")).unwrap();
        assert!(root.resolve(&["escape".into()]).is_err());
        assert_eq!(root.directory_names(&[], 4).unwrap(), ["alias", "inside"]);
        assert_eq!(
            root.resolve(&["alias".into()]).unwrap(),
            path.join("inside").canonicalize().unwrap()
        );
    }
    let moved = temp.path().join("previous-root");
    #[cfg(windows)]
    {
        // cap-std excludes FILE_SHARE_DELETE for open directory handles.
        // Windows therefore prevents rebinding instead of detecting it later.
        assert_eq!(
            fs::rename(&path, &moved).unwrap_err().raw_os_error(),
            Some(32)
        );
        root.validate(&path).unwrap();
        assert_eq!(root.directory_names(&[], 2).unwrap(), ["inside"]);
        drop(root);
        fs::rename(&path, &moved).unwrap();
    }
    #[cfg(unix)]
    {
        fs::rename(&path, &moved).unwrap();
        fs::create_dir_all(path.join("replacement")).unwrap();
        assert!(
            root.resolve(&[]).is_err(),
            "path reuse does not republish a new root"
        );
        assert!(root.directory_names(&[], 10).is_err());
        assert!(root.validate(&path).is_err());
    }
}
