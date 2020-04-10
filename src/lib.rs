use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command};
use std::thread::{self, JoinHandle};
use std::{env, fmt};

#[derive(Clone, Debug)]
pub enum BuildMode {
    // /// Build the listed non-main packages into .a files. Packages named
    // /// main are ignored.
    //Archive,

    /// Build the listed main package, plus all packages it imports,
    /// into a C archive file. The only callable symbols will be those
    /// functions exported using a cgo //export comment. Requires
    /// exactly one main package to be listed.
    CArchive,

    /// Build the listed main package, plus all packages it imports,
    /// into a C shared library. The only callable symbols will
    /// be those functions exported using a cgo //export comment.
    /// Requires exactly one main package to be listed.
    CShared,

    // /// Listed main packages are built into executables and listed
    // /// non-main packages are built into .a files (the default
    // ///  behavior)
    //Default,

    // /// Combine all the listed non-main packages into a single shared
    // /// library that will be used when building with the -linkshared
    // /// option. Packages named main are ignored.
    //Shared,

    // /// Build the listed main packages and everything they import into
    // /// executables. Packages not named main are ignored.
    //Exe,

    // /// Build the listed main packages and everything they import into
    // /// position independent executables (PIE). Packages not named
    // /// main are ignored.
    //Pie,

    // /// Build the listed main packages, plus all packages that they
    // ///import, into a Go plugin. Packages not named main are ignored.
    //Plugin,

    //Custom(String),
}

impl fmt::Display for BuildMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // BuildMode::Archive => write!(f, "archive"),
            BuildMode::CArchive => write!(f, "c-archive"),
            BuildMode::CShared => write!(f, "c-shared"),
            // BuildMode::Default => write!(f, "default"),
            // BuildMode::Shared => write!(f, "shared"),
            // BuildMode::Exe => write!(f, "exe"),
            // BuildMode::Pie => write!(f, "pie"),
            // BuildMode::Plugin => write!(f, "plugin"),
            // BuildMode::Custom(ref s) => write!(f, "{}", s),
        }
    }
}

impl Default for BuildMode {
    fn default() -> Self {
        Self::CArchive
    }
}

/// A builder for compilation of a native golang project.
///
/// A `Build` is the main type of the `gobuild` crate and is used to control all the
/// various configuration options and such of a compile. You'll find more
/// documentation on each method itself.
#[derive(Clone, Debug, Default)]
pub struct Build {
    files: Vec<PathBuf>,
    out_dir: Option<PathBuf>,
    buildmode: BuildMode,
    cargo_metadata: bool,
}

/// Represents the types of errors that may occur.
#[derive(Clone, Debug)]
enum ErrorKind {
    EnvVarNotFound,
    ToolNotFound,
    ToolExecError,
}

/// Represents an internal error that occurred, with an explanation.
#[derive(Clone, Debug)]
pub struct Error {
    /// Describes the kind of error that occurred.
    kind: ErrorKind,
    /// More explanation of the error that occurred.
    message: String,
}

impl Error {
    fn new(kind: ErrorKind, message: &str) -> Self {
        Self {
            kind,
            message: message.to_owned(),
        }
    }
}

impl Build {
    /// Construct a new instance of a blank set of configuration.
    ///
    /// This builder is finished with the [`compile`] function.
    ///
    /// [`compile`]: struct.Build.html#method.compile
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            out_dir: None,
            buildmode: BuildMode::CArchive,
            cargo_metadata: true,
        }
    }

    /// Add a file which will be compiled
    pub fn file<P: AsRef<Path>>(&mut self, p: P) -> &mut Build {
        self.files.push(p.as_ref().to_path_buf());
        self
    }

    /// Add files which will be compiled
    pub fn files<P>(&mut self, p: P) -> &mut Build
    where
        P: IntoIterator,
        P::Item: AsRef<Path>,
    {
        for file in p.into_iter() {
            self.file(file);
        }
        self
    }

    /// Configures the output directory where all object files and
    /// libraries will be located.
    ///
    /// This option is automatically scraped from the `OUT_DIR` environment
    /// variable by build scripts, so it's not required to call this function.
    pub fn out_dir<P: AsRef<Path>>(&mut self, out_dir: P) -> &mut Build {
        self.out_dir = Some(out_dir.as_ref().to_owned());
        self
    }

    /// Configures the build mode. See `go help buildmode for more details.
    ///
    /// Build mode `c-archive` is used by default.
    pub fn buildmode(&mut self, buildmode: BuildMode) -> &mut Build {
        self.buildmode = buildmode;
        self
    }

    /// Define whether metadata should be emitted for cargo allowing it to
    /// automatically link the binary. Defaults to `true`.
    ///
    /// The emitted metadata is:
    ///
    ///  - `rustc-link-lib=static=`*compiled lib*
    ///  - `rustc-link-search=native=`*target folder*
    ///
    pub fn cargo_metadata(&mut self, cargo_metadata: bool) -> &mut Build {
        self.cargo_metadata = cargo_metadata;
        self
    }

    /// Run the compiler, generating the file `output`
    ///
    /// This will return a result instead of panicing; see compile() for the complete description.
    pub fn try_compile(&self, lib_name: &str) -> Result<(), Error> {
        let gnu_lib_name = self.get_gnu_lib_name(lib_name);
        let dst = self.get_out_dir()?;
        let out = dst.join(&gnu_lib_name);

        let mut command = process::Command::new("go");
        command.arg("build");
        command.args(&["-buildmode", &self.buildmode.to_string()]);
        command.args(&["-o", &out.display().to_string()]);
        command.args(self.files.iter());

        run(&mut command, lib_name)?;

        match self.buildmode {
            BuildMode::CArchive => self.println(&format!("cargo:rustc-link-lib=static={}", lib_name)),
            BuildMode::CShared => self.println(&format!("cargo:rustc-link-lib=dylib={}", lib_name)),
        }
        self.println(&format!("cargo:rustc-link-search=native={}", dst.display()));
        Ok(())
    }

    /// Run the compiler, generating the file `output`
    ///
    /// The name `output` should be the name of the library. The Rust compilier will create
    /// the assembly with the lib prefix and .a extension.
    ///
    /// # Panics
    ///
    /// Panics if `output` is not formatted correctly or if one of the underlying
    /// compiler commands fails. It can also panic if it fails reading file names
    /// or creating directories.
    pub fn compile(&self, output: &str) {
        if let Err(e) = self.try_compile(output) {
            fail(&e.message);
        }
    }

    fn get_out_dir(&self) -> Result<PathBuf, Error> {
        let path = match self.out_dir.clone() {
            Some(p) => p,
            None => env::var_os("OUT_DIR").map(PathBuf::from).ok_or_else(|| {
                Error::new(
                    ErrorKind::EnvVarNotFound,
                    "Environment vairable OUT_DIR not defined.",
                )
            })?,
        };
        Ok(path)
    }

    fn get_gnu_lib_name(&self, lib_name: &str) -> String {
        let mut gnu_lib_name = String::with_capacity(5 + lib_name.len());
        gnu_lib_name.push_str("lib");
        gnu_lib_name.push_str(&lib_name);

        match self.buildmode {
            BuildMode::CArchive => gnu_lib_name.push_str(".a"),
            BuildMode::CShared => {
                if cfg!(windows) {
                    gnu_lib_name.push_str(".dll")
                } else {
                    gnu_lib_name.push_str(".so")
                }
            }
        }
        gnu_lib_name
    }

    fn println(&self, s: &str) {
        if self.cargo_metadata {
            println!("{}", s);
        }
    }
}

fn run(cmd: &mut Command, program: &str) -> Result<(), Error> {
    let (mut child, print) = spawn(cmd, program)?;
    let status = child.wait().map_err(|_| {
        Error::new(
            ErrorKind::ToolExecError,
            &format!(
                "Failed to wait on spawned child process, command {:?} with args {:?}",
                cmd, program
            ),
        )
    })?;
    print.join().unwrap();
    println!("{}", status);

    if status.success() {
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::ToolExecError,
            &format!(
                "Command {:?} with args {:?} did not execute successfully (status code {}).",
                cmd, program, status
            ),
        ))
    }
}

fn spawn(cmd: &mut Command, program: &str) -> Result<(Child, JoinHandle<()>), Error> {
    match cmd.stderr(process::Stdio::piped()).spawn() {
        Ok(mut child) => {
            let stderr = BufReader::new(child.stderr.take().unwrap());
            let print = thread::spawn(move || {
                for line in stderr.split(b'\n').filter_map(|l| l.ok()) {
                    print!("cargo:warning=");
                    io::stdout().write_all(&line).unwrap();
                    println!("");
                }
            });
            Ok((child, print))
        }
        Err(ref e) if e.kind() == io::ErrorKind::NotFound => Err(Error::new(
            ErrorKind::ToolNotFound,
            &format!("Failed to find tool.  Is {} installed?", program),
        )),
        Err(_) => Err(Error::new(
            ErrorKind::ToolExecError,
            &format!("Command {:?} with args {:?} failed to start.", cmd, program),
        )),
    }
}

fn fail(s: &str) -> ! {
    let _ = writeln!(io::stderr(), "\n\nerror occurred: {}\n\n", s);
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
