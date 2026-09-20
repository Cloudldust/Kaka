# 咔咔（Kaka）

[English](README.md) ｜ 简体中文

## 项目简介

咔咔是一个 Windows 桌面端的相机照片导入与快速筛选工具，功能只有两个：把照片导入本地数据库索引（复制模式从存储卡拷贝 / 添加模式原地索引），然后用全键盘操作快速过片——Q 标记待删、E 跳过、整组移入回收站，剩下的交给 Lightroom 做后期。

技术形态：Rust + egui 单可执行文件，SQLite 静态编译进二进制，双击即用，不依赖运行时或外部 DLL。所有照片文件只读或移入回收站，内容不会被修改。

## 功能列表

导入：

- 复制模式（从存储卡拷贝，保留原始文件名）与添加模式（仅建立索引）
- 递归扫描子文件夹；目标目录支持保持原结构 / 按拍摄日期 / 全部平铺三种组织方式，重名自动加 `_dup` 后缀且同名 XMP 侧车同步改名
- 导入前预扫描：去重标记（新 / 已存在 / 路径可修复）+ 可勾选文件网格
- 磁盘空间预检测（不足则阻断）、断点续传、全部成功后可清空存储卡
- 存储卡热插拔自动弹出导入窗口；拖入文件夹到窗口直接进入添加模式导入
- 导入完成报告（成功 / 失败 / 修复明细，失败列表可导出 CSV）

RAW+JPG 配对：

- 同目录同名且 EXIF 拍摄时间差不超过阈值（默认 5 秒，可调 1–30）自动配对
- 筛选界面配对合并显示为同一张（显示 RAW，带 R+J 角标）
- Q/E/U 整组标记、待删框整组删除或恢复、导出成对复制；其中一方丢失后下次启动自动解配对

筛选：

- 键盘操作：Q 待删 / E 已阅 / U 重置；方向键、A/D、空格切换
- Z 键 100% 放大：RAW 全帧后台解码、直方图影调对齐（RAW 与预览观感一致）、视口小地图、Ctrl+滚轮自由缩放、Ctrl+拖拽平移
- 旋转：R 顺转 / Ctrl+R 逆转 / Shift+R 重置为 EXIF 方向
- 搜索：300ms 防抖、回车即时应用；`@待删 / @已阅 / @未处理 / @丢失 / @配对` 前缀、`&& / || / !` 逻辑组合、`@` 自动补全
- 高级过滤：状态、相机、镜头、ISO、焦距、光圈、快门、日期范围、格式、丢失、配对
- 多选（Ctrl/Shift 单击、Ctrl+A）与批量标记（Ctrl+Q/E/U，二次确认）
- 撤销/重做（Ctrl+Z / Ctrl+Y，单键标记入栈，上限 100 步）
- 待删框：缩略图网格、配对折叠为删除单元、整组恢复或全部移入回收站
- 直方图（RGB / 单通道切换）、高光暗部溢出提示、数字跳片、筛选完成提示
- 拖入图片文件：同属一个文件夹时自动切换工作区并定位第一张

导出：

- 复制保留照片（三种组织方式，可选携带原 XMP 侧车与旋转信息，后台执行）
- 生成保留照片清单（.txt / .csv）
- 写 XMP 标记（Kaka:Keep + 星级；RAW 写侧车，JPEG/PNG 额外内嵌进文件）
- 发送到 Lightroom 经典版（临时收藏夹 .lrtemplate；15 秒未启动成功会提示并给出打开文件夹按钮；未安装 LR 时入口灰显）

数据安全与维护：

- 数据库损坏时三按钮选择：自动修复（优先手动备份）/ 手动选择备份 / 放弃并新建
- 设置内提供完整性检查、手动备份（保留 5 份）、恢复备份
- 非正常关闭后启动提供崩溃恢复选择；未完成导入可断点续传
- 删除操作只进回收站，不物理删除

界面与设置：

- 首次启动三步引导（欢迎 / 基础设置 / 选择导入，可跳过，只弹一次）
- 顶栏路径下拉（最近 10 个文件夹、浏览、复制路径）、Ctrl+L 直接输入路径
- 12 个核心动作可自定义键位（冲突检测、恢复默认、保存即生效）
- 中英文界面切换、F11 无边框全屏、最小窗口 1024×640、Per-Monitor V2 DPI
- 缓存：磁盘缩略图与预览两级缓存、后台预解码、2GB 内存 LRU、路径可自定义并迁移
- 日志：`%APPDATA%/Kaka/logs/`，纯文本，保留 14 天

支持格式：

- RAW：NEF/NRW、CR2/CR3、ARW/SR2、RAF、PEF/PTX、ORF、RW2、DNG、IIQ、3FR、X3F 等
- JPEG、PNG、TIFF
- HEIC/HEIF：依赖系统「HEIF 图像扩展」，未安装时解码失败自动跳过
- 无法解码的 RAW 自动回退显示内嵌预览

已知限制：

- 单窗口单工作区，无多 Tab；无自动更新（GitHub Release 手动下载覆盖）
- TIFF/HEIF 的 XMP 标记只写侧车，不内嵌进文件
- 设置面板为单页滚动布局，不是多 Tab 侧边栏

## 构建步骤

环境要求：

- Windows 10 21H2+ / Windows 11（x86_64）
- Rust stable 工具链（edition 2024，rustup 安装即可）
- 无其他依赖：SQLite 由 rusqlite bundled 特性静态编译，不需要 VC++ 运行库或 .NET

```bash
git clone https://github.com/Cloudldust/Kaka
cd kaka
cargo build --release
```

产物为单个可分发可执行文件 `target/release/kaka.exe`。

注意：本仓库 `.gitignore` 排除了 `.cargo/`。若你维护了机器本地的 `.cargo/config.toml`（例如把 `target-dir` 重定向到其他盘），它只对你的机器生效且不会随仓库分发——新克隆的仓库按默认 `target/` 目录构建。

开发：

```bash
cargo run      # 编译并以调试模式运行（default features 已含 GUI）
cargo test     # 运行全部测试（单元 + 集成）
cargo check    # 仅做类型检查
```

## 配置说明

程序不需要环境变量或 API 密钥。所有用户配置保存在 `%APPDATA%/Kaka/config.toml`，由设置界面写入，也可手工编辑。示例：

```toml
language = "zh"                     # 界面语言：zh / en
auto_open_last_workspace = true     # 启动时自动打开上次工作区
auto_detect_card = true             # 检测到存储卡自动弹出导入窗口
default_target_dir = "D:/Photos"    # 复制模式默认目标目录
cache_dir = "D:/KakaCache"          # 缓存根目录（默认 %LOCALAPPDATA%/Kaka/cache）
cache_capacity_gb = 20              # 磁盘缓存容量上限
cache_expire_days = 30              # 缓存过期天数
star_rating = 3                     # 写 XMP 时的星级
pair_time_threshold_secs = 5        # RAW+JPG 配对时间差阈值（1–30 秒）
lr_install_path = ""                # Lightroom.exe 路径或所在目录，留空自动检测
export_space_guard = true           # 导出前磁盘空间预检测
include_sidecar_export = true       # 复制导出时携带原 XMP 侧车
github_repo = "https://github.com/Cloudldust/Kaka"  # 设置 → 关于里「打开 GitHub 仓库」的地址

[keybindings]                       # 键位覆盖：动作码 → 键码，缺省用内置默认
mark_delete = "Q"
mark_reviewed = "E"
rotate_cw = "R"
next_photo = "ArrowRight"
```

其他数据文件位置：

| 内容 | 路径 |
|------|------|
| 数据库 | `%APPDATA%/Kaka/kaka.db` |
| 配置 | `%APPDATA%/Kaka/config.toml` |
| 日志 | `%APPDATA%/Kaka/logs/` |
| 缓存 | `%LOCALAPPDATA%/Kaka/cache/`（可由 `cache_dir` 覆盖） |

可重映射的键位动作码：`mark_delete`、`mark_reviewed`、`mark_untreated`、`rotate_cw`、`next_photo`、`prev_photo`、`toggle_zoom`、`toggle_panel`、`select_all`、`undo`、`redo`、`save`。

## License

MIT License，见 [LICENSE](LICENSE)。