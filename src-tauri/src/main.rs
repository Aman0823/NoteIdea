// Windows release 构建下不弹控制台窗口。
// dev 构建保留控制台，因为延迟测量的日志要靠它看。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    note_idea_lib::run()
}
