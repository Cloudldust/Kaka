fn main() {
    #[cfg(feature = "gui")]
    {
        if let Err(e) = kaka::app::ui::run() {
            eprintln!("[kaka] 启动失败: {e}");
            std::process::exit(1);
        }
    }
    #[cfg(not(feature = "gui"))]
    {
        eprintln!("[kaka] 此构建未包含图形界面，请使用 `--features gui` 重新编译。");
    }
}
