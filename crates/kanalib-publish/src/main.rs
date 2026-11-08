//! kanalib-publish CLI：把 Kanalib 本地图库导出成云端图片墙的静态数据目录。
//!
//! 只读语义：不写库（打开时 core 会补列/建索引，与桌面行为一致），不改原图与缩略图。
//! 全部日志走 stderr，stdout 只留统计摘要，导出目录里只有契约规定的产物。

use std::path::PathBuf;
use std::process::ExitCode;

use kanalib_publish::{Options, run};

const USAGE: &str = "\
kanalib-publish —— 把 Kanalib 本地库导出成静态站点数据目录（云端图片墙 F-080）

用法:
  kanalib-publish --db <库文件> --out <导出目录> [选项]

必填:
  --db <PATH>           SQLite 库文件（打开时会执行 core 迁移：补列/建索引）
  --out <DIR>           导出目录（不存在会自动创建）

选项:
  --limit <N>           最多导出 N 条（调试用；缺省全量）
  --with-originals      额外导出原图到 o/<id>.<ext>（仅本地来源）
  --dry-run             只打印统计，不写任何文件
  --shard-size <N>      每片条数（默认 5000）
  --max-tags <N>        facets.tags 上限（默认 2000）
  --max-groups <N>      facets.groups 上限（默认 200）
  --root <DIR>          显式指定本地根目录，可重复；缺省读 settings 再回退 sources
  --prune-stale         删除未被本次 index.json 引用的陈旧分片（默认只警告）
  -h, --help            显示本帮助
";

fn main() -> ExitCode {
    match main_inner() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("[kanalib-publish] 错误：{message}");
            eprintln!("[kanalib-publish] 用 --help 查看用法");
            ExitCode::from(2)
        }
    }
}

fn main_inner() -> Result<ExitCode, String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        eprintln!("{USAGE}");
        return Ok(ExitCode::from(2));
    }
    let options = match parse_args(&args)? {
        Some(options) => options,
        None => {
            print!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
    };

    match run(&options) {
        Ok(stats) => {
            println!("{stats}");
            if options.dry_run {
                println!("（--dry-run：以上为预演统计，未写入任何文件）");
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!("[kanalib-publish] 发布失败：{error:#}");
            Ok(ExitCode::FAILURE)
        }
    }
}

/// 解析命令行；返回 Ok(None) 表示只需打印帮助。
fn parse_args(args: &[String]) -> Result<Option<Options>, String> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        index += 1;
        match arg {
            "-h" | "--help" => return Ok(None),
            "--with-originals" => options.with_originals = true,
            "--dry-run" => options.dry_run = true,
            "--prune-stale" => options.prune_stale = true,
            "--db" | "--out" | "--limit" | "--shard-size" | "--max-tags" | "--max-groups" | "--root" => {
                let Some(value) = args.get(index) else {
                    return Err(format!("{arg} 缺少参数值"));
                };
                index += 1;
                match arg {
                    "--db" => options.db_path = PathBuf::from(value),
                    "--out" => options.out_dir = PathBuf::from(value),
                    "--limit" => options.limit = Some(parse_usize(arg, value)?),
                    "--shard-size" => options.shard_size = parse_usize(arg, value)?,
                    "--max-tags" => options.max_facet_tags = parse_usize(arg, value)?,
                    "--max-groups" => options.max_facet_groups = parse_usize(arg, value)?,
                    "--root" => options.roots.push(PathBuf::from(value)),
                    _ => unreachable!("已在外层 match 覆盖"),
                }
            }
            other => return Err(format!("无法识别的参数：{other}")),
        }
    }

    if options.db_path.as_os_str().is_empty() {
        return Err("必须提供 --db".to_string());
    }
    if options.out_dir.as_os_str().is_empty() {
        return Err("必须提供 --out".to_string());
    }
    if options.shard_size == 0 {
        return Err("--shard-size 必须大于 0".to_string());
    }
    Ok(Some(options))
}

fn parse_usize(flag: &str, value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|error| format!("{flag} 需要非负整数，收到 {value}（{error}）"))
}
