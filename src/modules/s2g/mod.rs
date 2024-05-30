use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    thread::JoinHandle,
};

use constants::{
    CROWBAR_BINARY, GOLDSRC_SUFFIX, NO_VTF_BINARY, STUDIOMDL_BINARY, VTX_EXTENSION, VVD_EXTENSION,
};
use eyre::eyre;
use qc::{BodyGroup, Qc, QcCommand};
use smd::Smd;
use utils::fix_backslash;

use crate::modules::s2g::{
    qc_stuffs::create_goldsrc_base_qc_from_source, smd_stuffs::source_smd_to_goldsrc_smd,
    utils::find_file_with_ext_in_folder,
};

use self::{
    constants::MAX_TRIANGLE,
    utils::{
        maybe_add_extension_to_string, relative_to_less_relative, run_command_linux,
        run_command_linux_with_wine, run_command_windows,
    },
};

/// 1 detect mdl
/// decompile mdl
/// look at the mdl's linked files
/// check whether linked files exist
/// split triangles from smd file
/// create qc file
/// call studiomdl.exe
///
/// extra steps:
/// bmp conversion
mod constants;
mod qc_stuffs;
mod smd_stuffs;
mod utils;

pub struct S2GSettings {
    studiomdl: PathBuf,
    crowbar: PathBuf,
    no_vtf: PathBuf,
    wine_prefix: Option<String>,
}

impl Default for S2GSettings {
    fn default() -> Self {
        let current_exe_path = std::env::current_exe().unwrap();
        let path_to_dist = current_exe_path.parent().unwrap().join("dist");

        Self::new(&path_to_dist)
    }
}

// TODO: impl Default with included binaries.
impl S2GSettings {
    pub fn new(path_to_bin: &Path) -> Self {
        if !path_to_bin.exists() {
            panic!(
                "{} containing binaries for S2G does not exist.",
                path_to_bin.display()
            );
        }

        let studiomdl = path_to_bin.join(STUDIOMDL_BINARY);
        let crowbar = path_to_bin.join(CROWBAR_BINARY);
        let no_vtf = path_to_bin.join("no_vtf");
        let no_vtf = no_vtf.join(NO_VTF_BINARY);

        if !studiomdl.exists() {
            panic!(
                "Cannot find {} in {}",
                STUDIOMDL_BINARY,
                path_to_bin.display()
            );
        }

        if !crowbar.exists() {
            panic!(
                "Cannot find {} in {}",
                CROWBAR_BINARY,
                path_to_bin.display()
            );
        }

        if !no_vtf.exists() {
            panic!("Cannot find {} in {}", NO_VTF_BINARY, path_to_bin.display());
        }

        Self {
            studiomdl,
            crowbar,
            no_vtf,
            wine_prefix: None,
        }
    }

    pub fn studiomdl(&mut self, path: &str) -> &mut Self {
        self.studiomdl = path.into();
        self
    }

    pub fn crowbar(&mut self, path: &str) -> &mut Self {
        self.crowbar = path.into();
        self
    }

    pub fn no_vtf(&mut self, path: &str) -> &mut Self {
        self.no_vtf = path.into();
        self
    }

    pub fn wine_prefix(&mut self, path: &str) -> &mut Self {
        self.wine_prefix = Some(path.into());
        self
    }

    #[cfg(target_os = "linux")]
    fn check_wine_prefix(&self) -> eyre::Result<()> {
        if self.wine_prefix.is_none() {
            return Err(eyre!("No WINEPREFIX supplied"));
        }

        Ok(())
    }
}

pub struct S2GOptions {
    settings: S2GSettings,
    path: PathBuf,
    // steps
    decompile: bool,
    vtf: bool,
    smd_and_qc: bool,
    compile: bool,
    // other stuffs
    /// Proceeds even when there is failure
    force: bool,
    pseudo_stdout: String,
}

// TODO: fn new() without S2GSettings in the argument.
impl S2GOptions {
    pub fn new(path: &str) -> Self {
        Self {
            settings: S2GSettings::default(),
            path: PathBuf::from(path),
            decompile: true,
            vtf: true,
            smd_and_qc: true,
            compile: true,
            pseudo_stdout: String::new(),
            force: false,
        }
    }

    #[allow(dead_code)]
    pub fn new_with_path_to_bin(path: &str, path_to_bin: &str) -> Self {
        Self {
            settings: S2GSettings::new(PathBuf::from(path_to_bin).as_path()),
            path: PathBuf::from(path),
            decompile: true,
            vtf: true,
            smd_and_qc: true,
            compile: true,
            force: false,
            pseudo_stdout: String::new(),
        }
    }

    /// Decompiles Source model.
    pub fn decompile(&mut self, decompile: bool) -> &mut Self {
        self.decompile = decompile;
        self
    }

    /// Runs no_vtf to convert .vtf to .bmp.
    pub fn vtf(&mut self, vtf: bool) -> &mut Self {
        self.vtf = vtf;
        self
    }

    /// Converts .smd and .qc.
    pub fn smd_and_qc(&mut self, smd_and_qc: bool) -> &mut Self {
        self.smd_and_qc = smd_and_qc;
        self
    }

    /// Compiles the new GoldSrc model.
    pub fn compile(&mut self, compile: bool) -> &mut Self {
        self.compile = compile;
        self
    }

    pub fn set_wine_prefix(&mut self, wine_prefix: &str) -> &mut Self {
        self.settings.wine_prefix(wine_prefix);
        self
    }

    /// An amateurish way to instrumentation and proper logging.
    fn log(&mut self, what: &str) {
        println!("{}", what);
        self.pseudo_stdout += what;
    }

    /// Continues with the process even if there is error
    pub fn force(&mut self, force: bool) -> &mut Self {
        self.force = force;
        self
    }

    fn work_decompile(&mut self, input_files: &Vec<PathBuf>) -> eyre::Result<()> {
        let mut handles: Vec<JoinHandle<eyre::Result<Output>>> = vec![];

        for input_file in input_files.iter() {
            let mut err_str = String::new();

            let mut vvd_path = input_file.clone();
            vvd_path.set_extension(VVD_EXTENSION);

            let mut vtx_path = input_file.clone();
            vtx_path.set_extension(VTX_EXTENSION);

            if !vvd_path.exists() {
                err_str += format!("Cannot find VVD file for {}", input_file.display()).as_str();
            }

            if !vtx_path.exists() {
                err_str += format!("Cannot find VTX file for {}", input_file.display()).as_str();
            }

            self.log(err_str.as_str());

            if !self.force && !err_str.is_empty() {
                return Err(eyre!(err_str));
            }

            handles.push(run_crowbar(&input_file, &self.settings));
        }

        // // TODO: do something with the output
        for handle in handles {
            let res = handle.join();
        }

        Ok(())
    }

    fn work_vtf(&mut self) -> eyre::Result<()> {
        let folder_path = if self.path.is_dir() {
            &self.path
        } else {
            self.path.parent().unwrap()
        };

        let handle = run_no_vtf(folder_path, &self.settings);

        let _ = handle.join();

        Ok(())
    }

    fn work_smd_and_qc(&mut self, input_files: &Vec<PathBuf>) -> eyre::Result<Vec<PathBuf>> {
        let mut missing_qc: Vec<PathBuf> = vec![];
        let mut qc_paths: Vec<PathBuf> = vec![];
        let mut compile_able_qcs: Vec<PathBuf> = vec![];

        input_files.iter().for_each(|file| {
            let mut probable_qc = file.clone();
            probable_qc.set_extension("qc");

            if !probable_qc.exists() {
                missing_qc.push(probable_qc)
            } else {
                qc_paths.push(probable_qc)
            }
        });

        if !missing_qc.is_empty() {
            let mut err_str = String::new();

            err_str += "Cannot find some correspondings .qc files: \n";

            for missing in missing_qc {
                err_str += &missing.display().to_string();
                err_str += "\n";
            }

            self.log(&err_str);

            if !self.force {
                return Err(eyre!(err_str));
            }
        }

        // Qc file would be the top level. There is 1 Qc file and it will link to other Smd files.
        let mut goldsrc_qcs: Vec<(PathBuf, Qc)> = vec![];

        for qc_path in qc_paths.iter() {
            let source_qc = Qc::from_file(qc_path.display().to_string().as_str());

            if let Err(err) = &source_qc {
                let err_str = format!("Cannot load QC {}: {}", qc_path.display(), err.to_string());

                self.log(&err_str);

                if !self.force {
                    return Err(eyre!(err_str));
                }
            }

            let source_qc = source_qc.unwrap();
            let mut goldsrc_qc = create_goldsrc_base_qc_from_source(&source_qc);
            let linked_smds = find_linked_smd_path(&qc_path.parent().unwrap(), &source_qc);

            if let Err(err) = &linked_smds {
                let err_str = format!(
                    "Cannot find linked SMD for {}: {}",
                    qc_path.display(),
                    err.to_string()
                );

                self.log(&err_str);

                if !self.force {
                    return Err(eyre!(err_str));
                }
            }

            let linked_smds = linked_smds.unwrap();

            // new smd name will be formated as
            // <old smd name><goldsrc suffix><index>.smd
            // eg: old smd name is `what.smd` -> what_goldsrc0.smd
            // if it is sequence then it will only add the goldsrc suffix
            for SmdInfo {
                name: _,
                smd,
                is_body,
                path,
            } in linked_smds.iter()
            {
                let goldsrc_smds = source_smd_to_goldsrc_smd(smd);
                let smd_file_name = path.file_stem().unwrap().to_str().unwrap();

                for (index, smd) in goldsrc_smds.iter().enumerate() {
                    let smd_path_for_qc = if *is_body {
                        let name = format!("studio{}", index);
                        let smd_path_for_qc = path
                            .with_file_name(format!("{}{}{}", smd_file_name, GOLDSRC_SUFFIX, index))
                            .with_extension("smd");

                        goldsrc_qc.add_body(
                            name.as_str(),
                            smd_path_for_qc.display().to_string().as_str(),
                            false,
                            None,
                        );

                        smd_path_for_qc
                    } else {
                        let smd_path_for_qc = path
                            .with_file_name(format!("{}{}", smd_file_name, GOLDSRC_SUFFIX))
                            .with_extension("smd");
                        // TODO do something more than just idle
                        goldsrc_qc.add_sequence(
                            "idle",
                            smd_path_for_qc.display().to_string().as_str(),
                            vec![],
                        );

                        smd_path_for_qc
                    };

                    let smd_path_for_writing = qc_path.parent().unwrap().join(smd_path_for_qc);

                    match smd.write(smd_path_for_writing.display().to_string().as_str()) {
                        Ok(_) => {}
                        Err(err) => {
                            let err_str = format!("Cannot write SMD: {}", err.to_string());

                            self.log(&err_str);

                            if !self.force {
                                return Err(eyre!(err_str));
                            }
                        }
                    };
                }
            }

            // after writing all of the SMD, now it is time to write our QC
            // not only that, we also add the appropriate model name inside the QC
            let goldsrc_qc_path = qc_path
                .with_file_name(format!(
                    "{}{}",
                    qc_path.file_stem().unwrap().to_str().unwrap(),
                    GOLDSRC_SUFFIX
                ))
                .with_extension("qc");
            let goldsrc_mdl_path = goldsrc_qc_path.with_extension("mdl");

            goldsrc_qc.set_model_name(goldsrc_mdl_path.display().to_string().as_str());

            goldsrc_qcs.push((goldsrc_qc_path, goldsrc_qc));
        }

        if goldsrc_qcs.len() != qc_paths.len() {
            let err_str = format!(
                "Failed to process {}/{} .qc files",
                qc_paths.len() - goldsrc_qcs.len(),
                qc_paths.len()
            );

            self.log(&err_str);

            if !self.force {
                return Err(eyre!(err_str));
            }
        }

        for (path, qc) in goldsrc_qcs.iter() {
            match qc.write(path.display().to_string().as_str()) {
                Ok(()) => {
                    compile_able_qcs.push(path.clone());
                }
                Err(err) => {
                    let err_str = format!(
                        "Cannot write QC {}: {}",
                        path.display().to_string(),
                        err.to_string()
                    );

                    self.log(&err_str);

                    if !self.force {
                        return Err(eyre!(err_str));
                    }
                }
            }
        }

        Ok(compile_able_qcs)
    }

    fn work_compile(&mut self, compile_able_qcs: &Vec<PathBuf>) -> eyre::Result<Vec<PathBuf>> {
        let mut result: Vec<PathBuf> = vec![];
        let mut instr_msg = String::from("Compiling {} models: \n");

        compile_able_qcs.iter().for_each(|path| {
            instr_msg += path.display().to_string().as_str();
        });

        self.log(instr_msg.as_str());

        compile_able_qcs.iter().for_each(|path| {
            run_studiomdl(path, &self.settings);
        });

        let mut goldsrc_mdl_path = compile_able_qcs
            .iter()
            .map(|path| {
                let mut new_path = path.clone();
                new_path.set_extension("mdl");
                new_path
            })
            .collect::<Vec<PathBuf>>();

        result.append(&mut goldsrc_mdl_path);

        Ok(result)
    }

    /// Does all the work.
    ///
    /// Returns the path of converted models .mdl
    pub fn work(&mut self) -> eyre::Result<Vec<PathBuf>> {
        let input_files = if self.path.is_file() {
            vec![self.path.clone()]
        } else {
            find_file_with_ext_in_folder(&self.path, "mdl")?
        };

        #[cfg(target_os = "linux")]
        self.settings.check_wine_prefix()?;

        // TODO: decompile would not keep anything after ward, just know the result that it works
        if self.decompile {
            self.work_decompile(&input_files)?;
        }

        // TODO what the above
        if self.vtf {
            self.work_vtf()?;
        }

        let mut compile_able_qcs: Vec<PathBuf> = vec![];

        if self.smd_and_qc {
            let mut res = self.work_smd_and_qc(&input_files)?;
            compile_able_qcs.append(&mut res);
        }

        let mut result: Vec<PathBuf> = vec![];

        if self.compile {
            let mut res = self.work_compile(&compile_able_qcs)?;
            result.append(&mut res);
        }

        Ok(result)
    }
}

struct Texture(PathBuf);

/// All of information related to a model conversion process
struct Bucket {
    file_name: PathBuf,
    textures: Vec<Texture>,
    orig_qc: Qc,
    orig_smd: Vec<SmdType>,
    converted_qc: Qc,
    converted_smd: Vec<SmdType>,
}

enum SmdType {
    Body(Smd),
    Sequence(Smd),
}

fn run_crowbar(mdl: &Path, settings: &S2GSettings) -> JoinHandle<eyre::Result<Output>> {
    // Assume that all settings are valid.
    let crowbar = &settings.crowbar;

    // `./crowbar -p model.mdl`
    let command = vec![
        crowbar.display().to_string(),
        "-p".to_string(),
        mdl.display().to_string(),
    ];

    let output = if cfg!(target_os = "windows") {
        run_command_windows(command)
    } else {
        run_command_linux_with_wine(command, settings.wine_prefix.as_ref().unwrap().to_string())
    };

    output
}

fn run_studiomdl(qc: &Path, settings: &S2GSettings) -> JoinHandle<eyre::Result<Output>> {
    // Assume that all settings are valid.
    let studiomdl = &settings.studiomdl;

    // `./studiomdl file.qc`
    let command = vec![studiomdl.display().to_string(), qc.display().to_string()];

    let output = if cfg!(target_os = "windows") {
        run_command_windows(command)
    } else {
        run_command_linux_with_wine(command, settings.wine_prefix.as_ref().unwrap().to_string())
    };

    output
}

fn run_no_vtf(folder: &Path, settings: &S2GSettings) -> JoinHandle<eyre::Result<Output>> {
    // Assume that all settings are valid.
    let no_vtf = &settings.no_vtf;

    // `./no_vtf <path to input dir> --output-dir <path to input dir again> --ldr-format png --max-resolution 512 --min-resolution 16`
    let command = vec![
        no_vtf.display().to_string(),
        folder.display().to_string(),
        "--output-dir".to_string(),
        folder.display().to_string(),
        "--ldr-format".to_string(),
        "png".to_string(),
        "--max-resolution".to_string(),
        "512".to_string(),
    ];

    let output = if cfg!(target_os = "windows") {
        run_command_windows(command)
    } else {
        run_command_linux(command)
    };

    output
}

#[derive(Debug)]
struct SmdInfo {
    name: String,
    smd: Smd,
    is_body: bool,
    path: PathBuf,
}

fn find_linked_smd_path(root: &Path, qc: &Qc) -> eyre::Result<Vec<SmdInfo>> {
    let mut res: Vec<SmdInfo> = vec![];

    for command in qc.commands() {
        let (name, smd, is_body) = match command {
            QcCommand::Body(body) => (body.name.clone(), body.mesh.clone(), true),
            QcCommand::Sequence(sequence) => {
                (sequence.name.clone(), sequence.skeletal.clone(), false)
            }
            QcCommand::BodyGroup(BodyGroup { name: _, bodies }) => {
                // TODO maybe more than 1 body will mess this up
                let body = &bodies[0];
                (body.name.clone(), body.mesh.clone(), true)
            }
            _ => continue,
        };

        // the goal is to returned Smd type so here we will try to open those files
        let smd = maybe_add_extension_to_string(smd.as_str(), "smd");
        let smd = fix_backslash(smd.as_str());
        let smd_path = PathBuf::from(smd);
        let smd = Smd::from_file(
            relative_to_less_relative(root, smd_path.as_path())
                .display()
                .to_string()
                .as_str(),
        )?;

        res.push(SmdInfo {
            name,
            smd,
            is_body,
            path: smd_path,
        });
    }

    Ok(res)
}
