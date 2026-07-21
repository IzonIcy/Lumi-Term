fn main() {
    cc::Build::new()
        .file("native/lumi_chrome.c")
        .warnings(true)
        .compile("lumi_chrome");
}
