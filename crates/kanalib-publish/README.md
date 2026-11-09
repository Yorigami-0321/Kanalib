# kanalib-publish（F-080 云端图片墙发布器）

把 Kanalib 本地图库导出成可静态托管的「图片墙」数据目录：index.json + 分片 + 缩略图（可选原图）+ 增量账本。
只读语义：不写库、不改原图 / 缩略图、日志只走 stderr、导出目录里只有契约规定的产物。

## 用法

~~~
# 抽样导出（调试、给前端联调用）
cargo run -p kanalib-publish -- --db .kanalib/kanalib.db --out .kanalib/wall_export --limit 200

# 全量导出（含原图）
cargo run -p kanalib-publish -- --db .kanalib/kanalib.db --out .kanalib/wall_export_full --with-originals

# 只演练不落盘
cargo run -p kanalib-publish -- --db .kanalib/kanalib.db --out .kanalib/wall_export --dry-run
~~~

### 参数

| 参数 | 必填 | 说明 |
| --- | --- | --- |
| --db <PATH> | 是 | SQLite 库文件。打开时 core 会执行迁移（补列 / 建索引），与桌面行为一致 |
| --out <DIR> | 是 | 导出目录，不存在会自动创建 |
| --limit <N> | | 最多导出 N 条（按 Modified 降序取前 N 条） |
| --with-originals | | 额外导出原图到 o/<id>.<ext>（仅本地来源） |
| --dry-run | | 只打印统计，不写任何文件、不建目录 |
| --shard-size <N> | | 每片条数，默认 5000 |
| --max-tags <N> | | facets.tags 上限，默认 2000 |
| --max-groups <N> | | facets.groups 上限，默认 200 |
| --root <DIR> | | 显式本地根目录，可重复；缺省读 settings.library.local_roots，为空则回退 sources 表里 kind=local 的 locator |
| --prune-stale | | 删除未被本次 index.json 引用的陈旧分片（默认只警告，不删） |
| -h, --help | | 帮助 |

退出码：0 = 成功；1 = 发布失败（数据库打不开等）；2 = 参数错误。

## 产物布局

~~~
<out>/
  index.json              全局索引：count / shards / facets / paths
  m/0000.json             分片，每片最多 shard_size 条
  m/0001.json
  t512/<id>.jpg           详情 / lightbox 档（硬不变量：每条 item 必有）
  t256/<id>.jpg           网格档（源缺失时只写 t512，前端 404 回退）
  o/<id>.<ext>            原图，仅 --with-originals
  publish-ledger.json     增量账本：id -> {sha1, mtime, bytes, files}
~~~

- 文件名主干就是 item.id，不含任何本地路径信息。
- 日志 / 警告只打到 stderr；导出目录内不放日志与临时文件（写文件走同目录隐藏临时文件 + rename）。
- index.json 最后写：旧 index 在新分片全部就绪前一直指向旧数据，不会被读到半成品。

## 与契约的对应关系（docs/backlog/photo-wall-contract.md v1）

- §2 index.json：schema=1、generated_at（RFC3339 UTC）、count、shard_size、shards[{i, path, count}]（i 从 0 连续递增，path 形如 m/0000.json）、facets.tags（按 count 降序、最多 2000、category 沿用库内值）、facets.groups（最多 200）、paths 四个档位前缀（t1024 即使未产出也照写）。
- §3 分片：m/{i:04}.json = {schema, index, items}；每片条数 <= shard_size；分片顺序即展示顺序，等于 Db::search_page 的 SearchSort::Modified 降序，同 mtime 由 core 的 name / path 兜底。
- §4 item：id、w、h、n（basename）、rt（G/S/Q/E，库内小写归一化为大写，缺失记 G）、g（可选，assets.group 原值，空则不写键）、st（0-5）、fav、tags（[[name, category, score]]，score 0-1 保留 3 位小数，自动标签在前、人工标签在后并按名去重）、ph（16 位小写 hex，缺失不出现）、mtime（UTC 秒）、orig（原图小写扩展名，缺失不出现）。
- §5 id：sha1（40 位小写 hex）-> blake3（64 位）-> blake3(path_key::encode(path))；不合法值降级到下一个来源；只允许 [0-9a-z]，长度 40 或 64；跨次发布稳定。
- §6 增量：publish-ledger.json 记录每个 id 的源指纹（sha1、mtime、bytes）与上次写出的文件列表；三者未变且目标文件仍在就跳过复制；--dry-run 不写盘。

## 实现要点

- 清单统一走 kanalib-core 的 Db::search_page（本地 roots、Filter::All、空查询、Modified 降序、keyset 分页），排序与去重语义完全由 core 保证，不新写 SQL。
- t512 是硬不变量：源缩略图文件缺失时跳过该资产并 warn，不中止发布、不产生半条数据。
- t256 从缩略图同目录的 {stem}-t256.jpg 派生（DB 不存该路径），缺失只计 missing，不报错。
- 同一 id（重复 sha1）只发布第一条，其余计入 duplicates。
- 被保留的那条**没有**任何标签、而同内容副本打过标签时，用副本的标签补位（stats.tags_from_duplicates 计数）；两份都有标签时仍以被保留那条为准，不混批次。
- 陈旧分片（m/ 下比本次分片数多的文件）默认只警告并提示；清理必须显式 --prune-stale，遵循「绝不自动删除」。
- 全流程单遍扫描、流式写分片，facets 用计数器累计，内存只保留当前分片。

## 已知边界

- t1024 / t2048 档位未产出：paths 里照写前缀，前端 404 回退到 t512（契约允许）。
- rclone / 对象存储同步未接入（属 F-083）；本 crate 只负责产出站点根目录。
- 打开库会触发 core 迁移（补列 / 建索引）：发布是只读语义，但物理上仍会写 WAL；纯只读打开入口需要 core 侧提供 Db::open_readonly（本期未做）。
- 原图只对本地来源导出；immich:// 与 webdav:// 等远程只读路径直接跳过。
- facets.tags / facets.groups 只统计本次导出范围（用 --limit 抽样时会相应变少）。
- 库内 tag 类别原样透传；若库里有契约枚举之外的类别（例如 rating），会原样出现在 facets 与 item.tags 里。

## 测试

~~~
cargo test -p kanalib-publish
~~~

覆盖：id 三级回退与稳定性、非法哈希降级、rating 归一化、可选字段缺省不出现、facets 排序与截断、分片切分边界（0 条 / 恰好 shard_size / 多一条）、--dry-run 不写盘、账本二次运行跳过未变资产且不重写文件、缩略图缺失跳过、原图缺失降级、重复 id 去重、重复副本补标签、--limit 截断、导出目录内无绝对路径（含 t256/t512 文件名均为合法 id）。
