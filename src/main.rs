use std::{path::PathBuf, time::Duration};
use minacalc_rs::{Calc, OsuCalcExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::{fs, time};
use tracing::*;
use tracing_subscriber::{fmt, EnvFilter};
use std::path::{Path};
use dotenvy::{from_path, from_path_iter, var};
use fs_extra::dir::{copy as copy_dir, CopyOptions}; // recursive copy
use anyhow::{Context, Result};

const POLL_MS: u64 = 600;

#[derive(Serialize, Default)]
struct MsdOut {
    song: String,
    diff: String,
    overall: f32,
    stamina: f32,
    jumpstream: f32,
    handstream: f32,
    stream: f32,
    chordjack:f32,
    jacks: f32,
    technical: f32,
    rate: String, // "1.60"
}

#[derive(Deserialize)]
struct JsonV2 {
    beatmap: BeatmapV2,
    play: PlayV2,
    // mods also often exists at root on some builds:
    mods: Option<ModsV2>,
}
#[derive(Deserialize)]
struct BeatmapV2 { artist: Option<String>, title: Option<String>, version: Option<String> }
#[derive(Deserialize)]
struct PlayV2 { mods: ModsV2 }
#[derive(Deserialize)]
struct ModsV2 {
    name: Option<String>,
    // newer builds expose array  rate/speed_change too:
    array: Option<Vec<ModEntry>>,
    rate: Option<f32>,
}
#[derive(Deserialize)]
struct ModEntry {
    #[serde(default)]
    settings: ModSettings,
    rate: Option<f32>,
}
#[derive(Deserialize, Default)]
struct ModSettings {
    #[serde(default)]
    speed_change: Option<f32>,
}

/// Find a tosu.env: CLI `--tosu-env <path>`, then env `TOSU_ENV_PATH`,
/// then `./tosu.env`, then `../tosu.env`.
fn find_tosu_env() -> Option<PathBuf> {
    let mut args = std::env::args();
    while let Some(a) = args.next() {
        if a == "--tosu-env" {
            if let Some(p) = args.next() { return Some(PathBuf::from(p)); }
        }
    }
    if let Ok(p) = std::env::var("TOSU_ENV_PATH") { return Some(PathBuf::from(p)); }
    for cand in ["./tosu.env", "../tosu.env"] {
        let p = PathBuf::from(cand);
        if p.exists() { return Some(p); }
    }
    None
}

fn resolve_static_root_from_tosu_env() -> Result<PathBuf,anyhow::Error> {
    if let Some(env_path) = find_tosu_env() {
        // Try strict load first (file values override process env)
        if let Err(e) = from_path(&env_path) {
            // Fallback only grab STATIC_FOLDER_PATH, ignore bad lines
            if let Ok(iter) = from_path_iter(&env_path) {
                for item in iter {
                    if let Ok((k, v)) = item {
                        if k == "STATIC_FOLDER_PATH" {
                            std::env::set_var(&k, &v);
                            break;
                        }
                    }
                    // else: Err(_) => skip malformed line
                }
            } else {
                return Err(e).with_context(|| format!("loading tosu.env at {:?}", env_path));
            }
        }
        if let Ok(val) = var("STATIC_FOLDER_PATH") {
            let p = PathBuf::from(val);
            return Ok(if p.is_absolute() { p } else {
                env_path.parent().unwrap_or(Path::new(".")).join(p)
            });
        }
    }
    // lenient dev fallback
    Ok(PathBuf::from("overlay"))
}

/// If `<static_root>/MinaCalcOnOsu/index.html` is missing, copy `./overlay` there (non-destructive).
fn install_overlay_if_missing(static_root: &Path) -> anyhow::Result<()> {
    let dest = static_root.join("MinaCalcOnOsu");
    if dest.join("index.html").exists() {
        return Ok(());
    }
    // Copy ./overlay -> <STATIC_FOLDER_PATH>/MinaCalcOnOsu (recursive).
    fs_extra::dir::create_all(&dest, false).ok(); // ensure dir tree (best-effort).
    let mut opt = CopyOptions::new(); // overwrite=false, skip_exist=false, copy_inside=false by default.
    opt.overwrite = false;
    opt.copy_inside = true;   // copy contents of ./overlay into dest (not the folder itself)
    opt.content_only = true;
    copy_dir("overlay", &dest, &opt).map(|_| ()).map_err(|e| anyhow::anyhow!(e))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {

    let mut ticker = time::interval(Duration::from_millis(POLL_MS));
    
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).init();

    let static_root = resolve_static_root_from_tosu_env()?;
    tokio::fs::create_dir_all(static_root.join("MinaCalcOnOsu")).await.ok();

    if let Err(e) = install_overlay_if_missing(&static_root) {
        warn!(%e, "overlay install skipped");
    }
    
    let http = Client::new();
    let calc = Calc::new()?;

    // Recalc guard (sha1  truncated rate)
    let mut last_sha1: Option<String> = None;
   // beatmap+rate dedupe
    let mut last_key: Option<(String, String)> = None; // (sha1, rate_str)

    loop {
        // 1) Pull v2 JSON snapshot
        ticker.tick().await;
        let v2 = match http.get("http://127.0.0.1:24050/json/v2").send().await {
            Ok(r) => match r.json::<JsonV2>().await { Ok(j) => j, Err(e) => { warn!(%e, "parse /json/v2"); sleep(); continue; } }
            Err(e) => { warn!(%e, "GET /json/v2"); sleep(); continue; }
        };

        // labels
        let artist  = v2.beatmap.artist.as_deref().unwrap_or("");
        let title   = v2.beatmap.title.as_deref().unwrap_or("");
        let version = v2.beatmap.version.clone().unwrap_or_default();
        let song_full = if !artist.is_empty() || !title.is_empty() { format!("{artist} - {title}") } else { "Unknown Song".to_string() };

        // 2) Extract rate from json/v2
        let raw_rate = extract_rate_from_v2(&v2).unwrap_or(1.0);
        let rate_str = format!("{:.2}", raw_rate);
        // 3) Get current .osu
        let osu_bytes = match http.get("http://127.0.0.1:24050/files/beatmap/file").send().await {
            Ok(rsp) => match rsp.bytes().await { Ok(b) => b.to_vec(), Err(e) => { warn!(%e, "bytes() failed"); continue; } },
            Err(e) => { warn!(%e, "GET .osu failed"); continue; }
        };
        
        if osu_bytes.is_empty() { warn!("No bytes from beatmap file"); continue; }
        // dedupe by (content, rate_str)
        let sha1 = sha1_smol::Sha1::from(&osu_bytes).hexdigest();
        
        if last_sha1.as_deref() == Some(&sha1) {
            if last_key.as_ref().is_some_and(|(h, r)| h == &sha1 && r == &rate_str) {continue;}
        }

        last_sha1 = Some(sha1.clone());
        last_key = Some((sha1, rate_str.clone()));

        // parse string → notes
        let osu_str = match String::from_utf8(osu_bytes) {
            Ok(s) => s,
            Err(e) => { error!(%e, "invalid UTF8 .osu"); continue; }
        };

        // Build notes from the osu!mania 4K map and compute SSR *at the exact rate*. 
        // OsuCalcExt::to_notes_merged converts Beatmap → Vec<Note>, then Calc::calc_ssr runs at any float rate. :contentReference[oaicite:5]{index=5}
        let scores = match (|| -> anyhow::Result<minacalc_rs::SkillsetScores> {
            // parse & validate (uses rosu_map under the hood)
            let beatmap: rosu_map::Beatmap = rosu_map::from_str(&osu_str)
                .map_err(|e| anyhow::anyhow!("parse failed: {e}"))?;
                minacalc_rs::Calc::security_check(&beatmap)
                .map_err(|e| anyhow::anyhow!("security_check: {e}"))?;
                let notes = minacalc_rs::Calc::to_notes_merged(&beatmap)
                .map_err(|e| anyhow::anyhow!("to_notes_merged: {e}"))?;
                // 93.0 is the common Etterna score goal used for MSD
                Ok(calc.calc_ssr(&notes, raw_rate, 93.0)?)
        })() {
            Ok(s) => s,
            Err(e) => { error!(%e, "calc_ssr failed"); continue; }
        };

        // write msd.json
        let out = MsdOut {
            song: song_full.clone(),
            diff: version.clone(),
            overall: scores.overall,
            stamina: scores.stamina,
            jumpstream: scores.jumpstream,
            handstream: scores.handstream,
            stream: scores.stream,
            chordjack: scores.chordjack,
            jacks: scores.jackspeed,
            technical: scores.technical,
            rate: rate_str,
        };
        if let Err(e) = write_msd_json(&static_root, &out).await {
            warn!(%e, "failed to write msd.json");
        } else {
            info!("msd.json updated: {} [{}] @{}x", out.song, out.diff, out.rate);
        }

    sleep();
}
}

fn sleep() { tokio::spawn(async { time::sleep(Duration::from_millis(150)).await; }); }

fn extract_rate_from_v2(v2: &JsonV2) -> Option<f32> {
    // Prefer explicit fields if present (newer Tosu builds):
    v2.play.mods.rate
        .or(v2.play.mods.array.as_ref()
            .and_then(|a| a.get(0))
            .and_then(|m| m.rate.or(m.settings.speed_change)))
        // Some builds also echo a top-level `mods` with the same structure:
        .or(v2.mods.as_ref().and_then(|m| m.rate.or_else(|| {
            m.array.as_ref().and_then(|a| a.get(0)).and_then(|e| e.rate.or(e.settings.speed_change))
        })))
        // Fallback: derive from name (DT/NC 1.5, HT/DC 0.75)
        .or_else(|| {
            let s = v2.play.mods.name.as_deref().unwrap_or("");
            if s.contains("NC") || s.contains("DT") { Some(1.5) }
            else if s.contains("HT") || s.contains("DC") { Some(0.75) }
            else { Some(1.0) }
        })
}

async fn write_msd_json(static_root: &PathBuf, out: &MsdOut) -> anyhow::Result<()> {
    let path = static_root.join("MinaCalcOnOsu").join("msd.json");
    if let Some(dir) = path.parent() { fs::create_dir_all(dir).await.ok(); }
    fs::write(&path, serde_json::to_vec(out)?).await?;
    Ok(())
}
