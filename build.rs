extern crate bindgen;

use std::{env, path::Path, process::Command};
use std::{fs, path::PathBuf};
use tempdir::TempDir;

#[cfg(target_os = "windows")]
const EXPECTED_JVM_FILENAME: &str = "jvm.dll";
#[cfg(any(
    target_os = "freebsd",
    target_os = "linux",
    target_os = "netbsd",
    target_os = "openbsd"
))]
const EXPECTED_JVM_FILENAME: &str = "libjvm.so";
#[cfg(target_os = "macos")]
const EXPECTED_JVM_FILENAME: &str = "libjli.dylib";

fn main() {
    let java_home = match env::var("JAVA_HOME") {
        Ok(java_home) => PathBuf::from(java_home),
        Err(_) => find_java_home().expect(
            "Failed to find Java home directory. \
             Try setting JAVA_HOME",
        ),
    };

    let libjvm_path =
        find_libjvm(&java_home).unwrap_or_else(|| panic!("Failed to find {}. Check JAVA_HOME", EXPECTED_JVM_FILENAME));

    println!("cargo:rustc-link-search=native={}", libjvm_path.display());

    // On Windows, we need additional file called `jvm.lib`
    // and placed inside `JAVA_HOME\lib` directory.
    if cfg!(windows) {
        let lib_path = java_home.join("lib");
        println!("cargo:rustc-link-search={}", lib_path.display());
    }

    println!("cargo:rerun-if-env-changed=JAVA_HOME");

    // On MacOS, we need to link to libjli instead of libjvm as a workaround
    // to a Java8 bug. See here for more information:
    // https://bugs.openjdk.java.net/browse/JDK-7131356
    if cfg!(target_os = "macos") {
        println!("cargo:rustc-link-lib=dylib=jli");
    } else {
        println!("cargo:rustc-link-lib=dylib=jvm");
    }

    println!("cargo:rerun-if-changed=wrapper.h");

    let mut builder = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg("-Ijdk/src/hotspot/share/include");

    builder = create_java_base_includes(builder);

    if cfg!(target_os = "windows") {
        builder = builder
            .clang_arg("-Ijdk/src/java.base/windows/native/include")
            .clang_arg("-Ijdk/src/hotspot/os/windows/include")
            .clang_arg("-Ijdk/src/java.base/windows/native/libjli");
    }

    if cfg!(target_os = "posix") {
        builder = builder.clang_arg("-Ijdk/src/hotspot/os/posix/include")
    }

    if cfg!(target_family = "unix") {
        builder = builder
            .clang_arg("-Ijdk/src/java.base/unix/native/include")
            .clang_arg("-Ijdk/src/java.base/unix/native/libjli");
    }

    if cfg!(target_os = "aix") {
        builder = builder.clang_arg("-Ijdk/src/java.base/aix/native/libjli");
    }

    if cfg!(feature = "desktop") {
        builder = builder.clang_arg("-Ijdk/src/java.desktop/share/native/include");

        if cfg!(target_os = "windows") {
            builder = builder.clang_arg("-Ijdk/src/java.desktop/windows/native/include");
        }

        if cfg!(target_os = "macos") {
            builder = builder.clang_arg("-Ijdk/src/java.desktop/macosx/native/include");
        }

        if cfg!(target_family = "unix") {
            builder = builder.clang_arg("-Ijdk/src/java.desktop/unix/native/include");
        }
    }

    if cfg!(feature = "jdwp") {
        builder = builder.clang_arg("-Ijdk/src/jdk.jdwp.agent/share/native");
    }

    if cfg!(feature = "accessibility") && cfg!(target_os = "windows") {
        builder = builder.clang_arg("-Ijdk/src/jdk.accessibility/windows/native/include");
    }

    // Workaround: We define this type as opaque because of errors caused by the default representation.
    builder = builder.opaque_type("_IMAGE_TLS_DIRECTORY64");

    let bindings = builder
        .parse_callbacks(Box::new(bindgen::CargoCallbacks))
        .generate()
        .expect("Unable to generate bindings");

    // Write the bindings to the $OUT_DIR/bindings.rs file.
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings!");
}

fn create_java_base_includes(builder: bindgen::Builder) -> bindgen::Builder {
    let temp_dir = TempDir::new("openjdk-sys-build").expect("Cannot create temporary build directory");
    let path = temp_dir.path();

    copy("jdk/src/java.base/share/native/include/", path).expect("Cannot copy java.base includes");

    let template_file = path.join("classfile_constants.h.template");
    let non_template_file = path.join("classfile_constants.h");
    fs::rename(template_file, non_template_file).expect("Cannot rename template file to non-template file");

    let path = format!("{}", temp_dir.path().display());

    // Workaround for to early deletion
    std::mem::forget(temp_dir);

    builder.clang_arg(format!("-I{}", path))
}

/// To find Java home directory, we call
/// `java -XshowSettings:properties -version` command and parse its output to
/// find the line `java.home=<some path>`.
fn find_java_home() -> Option<PathBuf> {
    Command::new("java")
        .arg("-XshowSettings:properties")
        .arg("-version")
        .output()
        .ok()
        .and_then(|output| {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            for line in stdout.lines().chain(stderr.lines()) {
                if line.contains("java.home") {
                    let pos = line.find('=').unwrap() + 1;
                    let path = line[pos..].trim();
                    return Some(PathBuf::from(path));
                }
            }
            None
        })
}

fn find_libjvm<S: AsRef<Path>>(path: S) -> Option<PathBuf> {
    let walker = walkdir::WalkDir::new(path).follow_links(true);

    for entry in walker {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_e) => continue,
        };

        let file_name = entry.file_name().to_str().unwrap_or("");

        if file_name == EXPECTED_JVM_FILENAME {
            return entry.path().parent().map(Into::into);
        }
    }

    None
}

fn copy<U: AsRef<Path>, V: AsRef<Path>>(from: U, to: V) -> Result<(), std::io::Error> {
    let mut stack = Vec::new();
    stack.push(PathBuf::from(from.as_ref()));

    let output_root = PathBuf::from(to.as_ref());
    let input_root = PathBuf::from(from.as_ref()).components().count();

    while let Some(working_path) = stack.pop() {
        println!("process: {:?}", &working_path);

        // Generate a relative path
        let src: PathBuf = working_path.components().skip(input_root).collect();

        // Create a destination if missing
        let dest = if src.components().count() == 0 {
            output_root.clone()
        } else {
            output_root.join(&src)
        };
        if fs::metadata(&dest).is_err() {
            println!(" mkdir: {:?}", dest);
            fs::create_dir_all(&dest)?;
        }

        for entry in fs::read_dir(working_path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                match path.file_name() {
                    Some(filename) => {
                        let dest_path = dest.join(filename);
                        println!("  copy: {:?} -> {:?}", &path, &dest_path);
                        fs::copy(&path, &dest_path)?;
                    },
                    None => {
                        println!("failed: {:?}", path);
                    },
                }
            }
        }
    }

    Ok(())
}
