use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
const AM_FINAL_MDL: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/am/final.mdl");
const CONF_MFCC: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/conf/mfcc.conf");
const CONF_MODEL: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/conf/model.conf");
const GRAPH_DISAMBIG: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/graph/disambig_tid.int");
const GRAPH_GR_FST: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/graph/Gr.fst");
const GRAPH_HCLR_FST: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/graph/HCLr.fst");
const GRAPH_PHONES_WORD_BOUNDARY: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/graph/phones/word_boundary.int");
const IVECTOR_FINAL_DUBM: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/ivector/final.dubm");
const IVECTOR_FINAL_IE: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/ivector/final.ie");
const IVECTOR_FINAL_MAT: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/ivector/final.mat");
const IVECTOR_GLOBAL_CMVN: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/ivector/global_cmvn.stats");
const IVECTOR_ONLINE_CMVN: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/ivector/online_cmvn.conf");
const IVECTOR_SPLICE: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/ivector/splice.conf");
const README: &[u8] = include_bytes!("../vosk-models/vosk-model-small-en-us-0.15/README");
pub fn extract_embedded_model() -> Result<PathBuf, String> {
    let temp_dir = std::env::temp_dir().join("theta-vosk-model-v1");
    let model_dir = temp_dir.join("vosk-model-small-en-us-0.15");
    if model_dir.exists() && model_dir.join("am/final.mdl").exists() {
        return Ok(model_dir);
    }
    fs::create_dir_all(&model_dir)
        .map_err(|e| format!("Failed to create model directory: {}", e))?;
    fs::create_dir_all(model_dir.join("am"))
        .map_err(|e| format!("Failed to create am directory: {}", e))?;
    fs::create_dir_all(model_dir.join("conf"))
        .map_err(|e| format!("Failed to create conf directory: {}", e))?;
    fs::create_dir_all(model_dir.join("graph"))
        .map_err(|e| format!("Failed to create graph directory: {}", e))?;
    fs::create_dir_all(model_dir.join("graph/phones"))
        .map_err(|e| format!("Failed to create graph/phones directory: {}", e))?;
    fs::create_dir_all(model_dir.join("ivector"))
        .map_err(|e| format!("Failed to create ivector directory: {}", e))?;
    write_file(&model_dir.join("am/final.mdl"), AM_FINAL_MDL)?;
    write_file(&model_dir.join("conf/mfcc.conf"), CONF_MFCC)?;
    write_file(&model_dir.join("conf/model.conf"), CONF_MODEL)?;
    write_file(&model_dir.join("graph/disambig_tid.int"), GRAPH_DISAMBIG)?;
    write_file(&model_dir.join("graph/Gr.fst"), GRAPH_GR_FST)?;
    write_file(&model_dir.join("graph/HCLr.fst"), GRAPH_HCLR_FST)?;
    write_file(&model_dir.join("graph/phones/word_boundary.int"), GRAPH_PHONES_WORD_BOUNDARY)?;
    write_file(&model_dir.join("ivector/final.dubm"), IVECTOR_FINAL_DUBM)?;
    write_file(&model_dir.join("ivector/final.ie"), IVECTOR_FINAL_IE)?;
    write_file(&model_dir.join("ivector/final.mat"), IVECTOR_FINAL_MAT)?;
    write_file(&model_dir.join("ivector/global_cmvn.stats"), IVECTOR_GLOBAL_CMVN)?;
    write_file(&model_dir.join("ivector/online_cmvn.conf"), IVECTOR_ONLINE_CMVN)?;
    write_file(&model_dir.join("ivector/splice.conf"), IVECTOR_SPLICE)?;
    write_file(&model_dir.join("README"), README)?;
    Ok(model_dir)
}
fn write_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut file = fs::File::create(path)
        .map_err(|e| format!("Failed to create file {}: {}", path.display(), e))?;
    file.write_all(content)
        .map_err(|e| format!("Failed to write to file {}: {}", path.display(), e))?;
    Ok(())
}
