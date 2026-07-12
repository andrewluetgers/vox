fn main() {
    if std::env::var_os("PIPER_ESPEAKNG_DATA_DIRECTORY").is_none() {
        let cache = dirs::cache_dir().unwrap().join("vox");
        if cache.join("espeak-ng-data").exists() {
            std::env::set_var("PIPER_ESPEAKNG_DATA_DIRECTORY", &cache);
        }
    }
    for lang in ["en-gb", "en", "en-gb-x-rp", "en-us"] {
        match espeak_rs::text_to_phonemes("Much better indeed.", lang, None, true, false) {
            Ok(p) => println!("{lang}: OK {:?}", p),
            Err(e) => println!("{lang}: ERROR {e}"),
        }
    }
}
