pub(crate) const ZH: &[(&str, &str)] = &[
    ("notice.library.add_folder", "请添加文件夹"),
    ("notice.library.ready_scan", "准备扫描"),
    ("notice.library.loaded", "已加载 {n} 张图片"),
    (
        "notice.library.restore_applied",
        "已应用备份 {path} · 请检查图库",
    ),
    (
        "notice.library.removed_ignored_hash",
        "已从图库移除 {removed} 张并忽略其路径与内容哈希 · 磁盘文件未删除",
    ),
    (
        "notice.library.removed_ignored_path",
        "已从图库移除 {removed} 张并忽略路径 · 磁盘文件未删除",
    ),
    ("notice.sources.added", "已添加 {n} 个来源"),
    (
        "notice.sources.removed",
        "已移除来源 · 磁盘文件未删除 · 剩余 {sources} 个来源",
    ),
    ("notice.scan.started", "扫描中..."),
    ("notice.scan.failed", "索引失败"),
    ("notice.scan.stopped", "已停止"),
    (
        "notice.scan.stopped_summary",
        "已停止 · 新增 {imported} · 更新 {updated} · 跳过 {skipped} · 失败 {failed} · {elapsed}s",
    ),
    (
        "notice.scan.complete",
        "完成 · {total} 张图片 · {sources} 个来源 · 新增 {imported} · 更新 {updated} · 跳过 {skipped} · 失败 {failed} · {elapsed}s",
    ),
    (
        "notice.scan.complete_queue",
        "完成 · {total} 张图片 · 队列 {pending} 张待处理 · {sources} 个来源 · 新增 {imported} · 更新 {updated} · 跳过 {skipped} · 失败 {failed} · {elapsed}s",
    ),
    (
        "notice.scan.complete_errors",
        "完成 · {total} 张图片 · {queue_failed} 个失败待处理 · {sources} 个来源 · 新增 {imported} · 更新 {updated} · 跳过 {skipped} · {elapsed}s",
    ),
    (
        "notice.scan.complete_errors_simple",
        "完成 · {total} 张图片 · {failed} 个失败待处理",
    ),
    (
        "notice.scan.complete_all_tagged",
        "完成 · {total} 张图片，全部已打标",
    ),
    (
        "notice.scan.complete_skip_tag",
        "完成 · {total} 张图片（跳过打标，队列 {pending} 张待处理）",
    ),
    (
        "notice.scan.complete_tag_paused",
        "完成 · {total} 张图片（打标已暂停，队列 {pending} 张待处理）",
    ),
    (
        "notice.scan.complete_tag_cancelled",
        "完成 · {total} 张图片（打标已取消，队列 {pending} 张未处理）",
    ),
    ("notice.scan.source_started", "扫描来源 · {source}"),
    (
        "notice.scan.source_started_multi",
        "扫描来源 {position}/{total} · {source}",
    ),
    (
        "notice.scan.source_complete",
        "来源扫描完成 · {source} · {total} 张图片 · 新增 {imported} · 更新 {updated} · 跳过 {skipped} · 失败 {failed} · {elapsed}s",
    ),
    ("notice.scan.source_failed", "来源扫描失败"),
    ("notice.scan.rescan_started", "重新扫描来源 · {source}"),
    ("notice.scan.rescan_failed", "重扫失败"),
    (
        "notice.scan.progress",
        "扫描来源 · {source} · {phase} · {done}/{total}",
    ),
    (
        "notice.scan.cleanup_removed",
        "已移除 {n} 张磁盘上已不存在的图片",
    ),
    (
        "notice.scan.tag_queue_restored",
        "待打标 {pending} 张 · 队列已恢复",
    ),
    ("notice.source.offline", "来源离线 · {path}"),
    (
        "notice.source.unreachable",
        "来源不可访问：{path}",
    ),
    (
        "notice.source.unreachable_kept",
        "来源不可访问，已保留图库记录：{path}",
    ),
    (
        "notice.source.incomplete_walk",
        "来源未能完整遍历（可能离线或权限不足），已保留图库记录：{path}",
    ),
    (
        "notice.source.offline_kept",
        "来源离线或不可访问，已保留现有记录：{path}",
    ),
    (
        "notice.source.watch_unavailable",
        "无法监听来源：{error}；将在后台自动重试",
    ),
    (
        "notice.source.offline_watch_retry",
        "来源离线，恢复后自动重试监听：{path}",
    ),
    (
        "notice.source.watch_io_error",
        "文件监听错误：{error}；请手动重新扫描该来源",
    ),
    (
        "notice.source.watch_batch_failed",
        "监听事件处理失败：{error}；请手动重新扫描该来源",
    ),
    (
        "notice.source.watch_storm_rescan_failed",
        "文件变化过多，自动重扫失败：{error}",
    ),
    (
        "notice.source.watch_sync_failed",
        "增量同步失败：{error}；请手动重新扫描该来源",
    ),
    ("notice.source.watch_recovered", "来源已恢复监听 · {source}"),
    ("notice.source.watch_failed", "文件监听失败 · {path}"),
    ("notice.source.watch_error", "文件监听异常 · {path}"),
    (
        "notice.source.watch_update",
        "监听更新 · 变更 {changed} · 移除 {removed} · 队列 {queue}",
    ),
    (
        "notice.source.scan_filter_updated",
        "已更新来源排除规则 · {path}",
    ),
    (
        "notice.source.scan_filter_saved_removed",
        "已保存排除规则 · 从图库移出 {removed} 张 · 磁盘文件未删除",
    ),
    (
        "notice.source.scan_filter_saved_hits",
        "已保存排除规则 · 已入库命中 {n} 张 · 保留记录",
    ),
    ("notice.source.alias_reset", "已恢复来源默认名称 · {path}"),
    ("notice.source.alias_set", "已重命名来源为 {alias}"),
    ("notice.ignore.added", "已添加忽略规则 · {pattern}"),
    (
        "notice.ignore.restored_paths",
        "已恢复 {restored} 条路径忽略项 · 重扫后重新导入",
    ),
    (
        "notice.ignore.restored_hashes",
        "已恢复 {restored} 条哈希忽略项 · 重扫后重新导入",
    ),
    ("notice.ignore.restored_glob", "已恢复忽略规则 · {pattern}"),
    ("notice.tags.added", "已为 {n} 张添加标签 #{name}"),
    ("notice.tags.removed", "已从 {n} 张移除标签 #{name}"),
    (
        "notice.tags.renamed",
        "已重命名标签 #{old} → #{new} · 影响 {n} 张",
    ),
    (
        "notice.tags.merged",
        "已合并标签 #{source} → #{target} · 影响 {n} 张",
    ),
    ("notice.tags.blocked", "已屏蔽标签 #{name}"),
    ("notice.tags.unblocked", "已取消屏蔽标签 #{name}"),
    ("notice.favorite.added", "已收藏 {n} 张"),
    ("notice.favorite.removed", "已取消收藏 {n} 张"),
    ("notice.star.cleared", "已清除 {n} 张的星级"),
    ("notice.star.set", "已设置 {n} 张为 {stars} 星"),
    ("notice.undo.favorite", "已撤销「{label}」· 恢复 {n} 张收藏"),
    ("notice.undo.star", "已撤销「{label}」· 恢复 {n} 张星级"),
    (
        "notice.undo.tag_removed",
        "已撤销「{label}」· 移除手工标签 #{name} · {n} 张",
    ),
    (
        "notice.undo.tags_restored",
        "已撤销「{label}」· 恢复 {n} 张的标签",
    ),
    (
        "notice.tag.threshold_category",
        "{category} 阈值已设为 {value}",
    ),
    ("notice.tag.threshold_tag", "标签 #{tag} 阈值已设为 {value}"),
    ("notice.tag.threshold_reset", "标签 #{tag} 已恢复类别阈值"),
    (
        "notice.tag.recomputed",
        "阈值重算完成 · {updated} 张更新 · {skipped} 张缺候选",
    ),
    ("notice.tag.candidate_accepted", "已接受候选 #{tag}"),
    ("notice.tag.candidate_ignored", "已忽略候选 #{tag}"),
    ("notice.tag.candidate_pending", "候选 #{tag} 已恢复待审"),
    ("notice.tag.batch_reviewed", "已批量处理 {changed} 条候选"),
    (
        "notice.tag.retag",
        "重打标 · {recomputed} 张按候选重算 · {queued} 张已入队",
    ),
    (
        "notice.tag_queue.paused",
        "打标已暂停 · 当前批完成后不再认领新任务",
    ),
    (
        "notice.tag_queue.cancelling",
        "正在取消打标 · 已完成结果会保留",
    ),
    ("notice.tag_queue.running", "打标已在运行"),
    ("notice.tag_queue.resuming", "继续打标..."),
    ("notice.tag_queue.failed_paused", "打标失败，已暂停"),
    (
        "notice.tag_queue.cpu_allowed",
        "已允许 CPU 慢速打标；继续队列后生效",
    ),
    ("notice.tag_queue.gpu_paused", "GPU 不可用时默认暂停打标"),
    (
        "notice.tag_queue.loading_model",
        "加载 PixAI 模型...（队列 {pending} 张）",
    ),
    (
        "notice.tag_queue.started",
        "打标中 0/{total} · {classes} 类标签",
    ),
    (
        "notice.tag_queue.gpu_unavailable_scan",
        "GPU 不可用（{device}）· 已暂停打标，队列 {pending} 张待处理；允许 CPU 慢速打标后可继续",
    ),
    (
        "notice.tag_queue.gpu_unavailable_worker",
        "GPU 不可用（{device}）· 已暂停打标；允许 CPU 慢速打标后可继续",
    ),
    (
        "notice.tag_queue.progress",
        "打标中 {done}/{total} · 队列 {pending} · 失败 {failed}",
    ),
    (
        "notice.tag_queue.cancelled",
        "已取消 · 已完成 {done}/{total} · 队列 {pending} 张未处理",
    ),
    (
        "notice.tag_queue.paused_queue",
        "打标已暂停 · 队列 {pending} 张待处理 · {failed} 个失败",
    ),
    (
        "notice.tag_queue.paused_pending",
        "打标暂停 · 队列 {pending} 张待处理 · {failed} 个失败",
    ),
    ("notice.tag_queue.complete", "完成 · {total} 张图片已打标"),
    ("notice.tag_queue.no_sources", "没有可打标的来源"),
    ("notice.tag_queue.no_jobs", "没有待打标任务"),
    ("notice.cleanup.kept", "已保留 {n} 张缺失图片记录"),
    (
        "notice.cleanup.removed",
        "已从图库移除 {removed} 张缺失图片记录",
    ),
    (
        "notice.cleanup.removed_kept",
        "已从图库移除 {removed} 张缺失记录，保留 {kept} 张已恢复文件",
    ),
    ("notice.repair.running", "修复中 · {label}"),
    ("notice.repair.failed", "修复失败 · {label}"),
    ("notice.repair.failed_plain", "修复失败"),
    (
        "notice.repair.cache_rebuilt_loading",
        "预览缓存已重建 · 加载打标模型...",
    ),
    (
        "notice.repair.cache_rebuilt_incomplete",
        "预览已重建，打标未完成",
    ),
    (
        "notice.repair.cache_rebuilt_retag",
        "预览已重建 · 重新打标...",
    ),
    ("notice.repair.done_damage", "修复完成（{damage}） · {name}"),
    ("notice.repair.done", "修复完成，文件可正常读取 · {name}"),
    (
        "notice.repair.incomplete",
        "修复未完成 · 文件仍不可完整读取",
    ),
    ("notice.repair.exporting", "正在导出修复副本 · {label}"),
    (
        "notice.repair.exported_damage",
        "已导出修复副本（{damage}） · {path}",
    ),
    ("notice.repair.exported", "已导出修复副本 · {path}"),
    ("notice.repair.export_failed", "导出修复副本失败"),
    ("notice.dedup.preselected", "已按规则预选 {selected} 组"),
    ("notice.dedup.marked_ignored", "已标记为不是重复"),
    ("notice.dedup.marked_related", "已标记为相关变体"),
    ("notice.dedup.marked_done", "已保留整组"),
    ("notice.dedup.marked_reopened", "已重新打开此组"),
    (
        "notice.dedup.similar_too_few",
        "图片不足 2 张，无法扫描相似图片",
    ),
    (
        "notice.dedup.similar_scanning",
        "相似扫描中 · 汉明距离 ≤{threshold} · {n} 张",
    ),
    (
        "notice.dedup.similar_stale",
        "图库已变化，请重新扫描相似图片",
    ),
    (
        "notice.dedup.similar_done",
        "相似扫描完成 · {groups} 组 · 未审 {pending} 组",
    ),
    ("notice.dedup.moved", "已移入回收站 {moved} 张"),
    (
        "notice.dedup.moved_partial",
        "已移入回收站 {moved} 张；{failed} 张仍待处理",
    ),
    (
        "notice.dedup.protected_skipped",
        "跳过 {skipped} 个受保护组，没有可移入回收站的副本",
    ),
    (
        "notice.dedup.batch_done",
        "批量处理完成：{groups} 组 · 移入回收站 {assets} 张",
    ),
    (
        "notice.dedup.batch_done_skipped",
        "批量处理完成：{groups} 组 · 移入回收站 {assets} 张；跳过 {skipped} 个受保护组",
    ),
    (
        "notice.dedup.protected_added_path",
        "已添加保护路径：{path}",
    ),
    ("notice.dedup.protected_added_entry", "已添加保护项：{path}"),
    ("notice.dedup.protected_removed", "已移除保护路径：{path}"),
    ("notice.saved_search.saved", "已保存智能集合「{name}」"),
    (
        "notice.saved_search.renamed",
        "已重命名智能集合为「{name}」",
    ),
    (
        "notice.saved_search.updated",
        "已用当前视图更新集合「{name}」",
    ),
    ("notice.saved_search.pinned", "已置顶智能集合"),
    ("notice.saved_search.unpinned", "已取消置顶智能集合"),
    ("notice.saved_search.reordered", "已调整智能集合顺序"),
    ("notice.saved_search.deleted", "已删除智能集合"),
    ("notice.album.created", "已新建相册「{name}」"),
    ("notice.album.renamed", "已重命名相册为「{name}」"),
    ("notice.album.deleted", "已删除相册（未删除图片）"),
    ("notice.album.added", "已向相册加入 {added} 张图片"),
    ("notice.album.removed", "已从相册移出 {removed} 张图片"),
    ("notice.remote.removed", "已移除来源「{name}」"),
    (
        "notice.remote.credentials_saved",
        "已保存 {name} 凭据（系统凭据库）",
    ),
    ("notice.remote.credentials_cleared", "已清除 {name} 凭据"),
    (
        "notice.remote.sync_done",
        "{name} 同步完成 · 新增 {imported} · 更新 {updated} · 跳过 {skipped} · 缩略图 {thumbs}",
    ),
    ("notice.remote.sync_failed", "{name} 同步失败：{message}"),
    ("notice.remote.syncing", "正在同步 {name}..."),
    ("notice.remote.sync_failed_plain", "远端来源同步失败"),
    ("notice.trash.records_failed", "回收站已移动 · 记录未删除"),
    ("notice.retry.failed", "重试失败"),
];

pub(crate) const EN: &[(&str, &str)] = &[
    ("notice.library.add_folder", "Add a folder to get started"),
    ("notice.library.ready_scan", "Ready to scan"),
    ("notice.library.loaded", "{n} images loaded"),
    (
        "notice.library.restore_applied",
        "Backup applied: {path} · check your library",
    ),
    (
        "notice.library.removed_ignored_hash",
        "Removed {removed} from library and ignored their paths and content hashes · files kept on disk",
    ),
    (
        "notice.library.removed_ignored_path",
        "Removed {removed} from library and ignored their paths · files kept on disk",
    ),
    ("notice.sources.added", "Added {n} sources"),
    (
        "notice.sources.removed",
        "Source removed · files kept on disk · {sources} sources remaining",
    ),
    ("notice.scan.started", "Scanning..."),
    ("notice.scan.failed", "Indexing failed"),
    ("notice.scan.stopped", "Stopped"),
    (
        "notice.scan.stopped_summary",
        "Stopped · added {imported} · updated {updated} · skipped {skipped} · failed {failed} · {elapsed}s",
    ),
    (
        "notice.scan.complete",
        "Done · {total} images · {sources} sources · added {imported} · updated {updated} · skipped {skipped} · failed {failed} · {elapsed}s",
    ),
    (
        "notice.scan.complete_queue",
        "Done · {total} images · {pending} queued · {sources} sources · added {imported} · updated {updated} · skipped {skipped} · failed {failed} · {elapsed}s",
    ),
    (
        "notice.scan.complete_errors",
        "Done · {total} images · {queue_failed} queued failures · {sources} sources · added {imported} · updated {updated} · skipped {skipped} · {elapsed}s",
    ),
    (
        "notice.scan.complete_errors_simple",
        "Done · {total} images · {failed} failed",
    ),
    (
        "notice.scan.complete_all_tagged",
        "Done · {total} images, all tagged",
    ),
    (
        "notice.scan.complete_skip_tag",
        "Done · {total} images (tagging skipped, {pending} queued)",
    ),
    (
        "notice.scan.complete_tag_paused",
        "Done · {total} images (tagging paused, {pending} queued)",
    ),
    (
        "notice.scan.complete_tag_cancelled",
        "Done · {total} images (tagging cancelled, {pending} pending)",
    ),
    ("notice.scan.source_started", "Scanning source · {source}"),
    (
        "notice.scan.source_started_multi",
        "Scanning source {position}/{total} · {source}",
    ),
    (
        "notice.scan.source_complete",
        "Source scan complete · {source} · {total} images · added {imported} · updated {updated} · skipped {skipped} · failed {failed} · {elapsed}s",
    ),
    ("notice.scan.source_failed", "Source scan failed"),
    ("notice.scan.rescan_started", "Rescanning source · {source}"),
    ("notice.scan.rescan_failed", "Rescan failed"),
    (
        "notice.scan.progress",
        "Scanning · {source} · {done}/{total}",
    ),
    (
        "notice.scan.cleanup_removed",
        "Removed {n} records missing on disk",
    ),
    (
        "notice.scan.tag_queue_restored",
        "{pending} images pending tagging · queue restored",
    ),
    ("notice.source.offline", "Source offline · {path}"),
    (
        "notice.source.unreachable",
        "Source unreachable: {path}",
    ),
    (
        "notice.source.unreachable_kept",
        "Source unreachable; library records kept: {path}",
    ),
    (
        "notice.source.incomplete_walk",
        "Source not fully walked (possibly offline or permission denied); library records kept: {path}",
    ),
    (
        "notice.source.offline_kept",
        "Source offline or unreachable; existing records kept: {path}",
    ),
    (
        "notice.source.watch_unavailable",
        "Cannot watch source: {error}; retrying in the background",
    ),
    (
        "notice.source.offline_watch_retry",
        "Source offline; watching resumes automatically when it returns: {path}",
    ),
    (
        "notice.source.watch_io_error",
        "File watching error: {error}; rescan the source manually",
    ),
    (
        "notice.source.watch_batch_failed",
        "Watch event processing failed: {error}; rescan the source manually",
    ),
    (
        "notice.source.watch_storm_rescan_failed",
        "Too many file changes; automatic rescan failed: {error}",
    ),
    (
        "notice.source.watch_sync_failed",
        "Incremental sync failed: {error}; rescan the source manually",
    ),
    (
        "notice.source.watch_recovered",
        "Source watching resumed · {source}",
    ),
    (
        "notice.source.watch_failed",
        "File watching failed · {path}",
    ),
    ("notice.source.watch_error", "File watching error · {path}"),
    (
        "notice.source.watch_update",
        "Watch update · {changed} changed · {removed} removed · {queue} queued",
    ),
    (
        "notice.source.scan_filter_updated",
        "Source exclusion rules updated · {path}",
    ),
    (
        "notice.source.scan_filter_saved_removed",
        "Exclusion rules saved · {removed} removed from library · files kept on disk",
    ),
    (
        "notice.source.scan_filter_saved_hits",
        "Exclusion rules saved · {n} indexed matches kept",
    ),
    ("notice.source.alias_reset", "Source name reset · {path}"),
    ("notice.source.alias_set", "Source renamed to {alias}"),
    ("notice.ignore.added", "Ignore rule added · {pattern}"),
    (
        "notice.ignore.restored_paths",
        "Restored {restored} path ignores · rescans will re-import",
    ),
    (
        "notice.ignore.restored_hashes",
        "Restored {restored} hash ignores · rescans will re-import",
    ),
    (
        "notice.ignore.restored_glob",
        "Ignore rule restored · {pattern}",
    ),
    ("notice.tags.added", "Tagged {n} images with #{name}"),
    ("notice.tags.removed", "Removed tag #{name} from {n} images"),
    (
        "notice.tags.renamed",
        "Renamed tag #{old} → #{new} · {n} images affected",
    ),
    (
        "notice.tags.merged",
        "Merged tag #{source} → #{target} · {n} images affected",
    ),
    ("notice.tags.blocked", "Blocked tag #{name}"),
    ("notice.tags.unblocked", "Unblocked tag #{name}"),
    ("notice.favorite.added", "Favorited {n} images"),
    ("notice.favorite.removed", "Unfavorited {n} images"),
    ("notice.star.cleared", "Cleared rating on {n} images"),
    ("notice.star.set", "Rated {n} images {stars} stars"),
    (
        "notice.undo.favorite",
        "Undid “{label}” · restored favorites on {n} images",
    ),
    (
        "notice.undo.star",
        "Undid “{label}” · restored ratings on {n} images",
    ),
    (
        "notice.undo.tag_removed",
        "Undid “{label}” · removed manual tag #{name} · {n} images",
    ),
    (
        "notice.undo.tags_restored",
        "Undid “{label}” · restored tags on {n} images",
    ),
    (
        "notice.tag.threshold_category",
        "{category} threshold set to {value}",
    ),
    (
        "notice.tag.threshold_tag",
        "Tag #{tag} threshold set to {value}",
    ),
    (
        "notice.tag.threshold_reset",
        "Tag #{tag} reset to category threshold",
    ),
    (
        "notice.tag.recomputed",
        "Threshold recompute done · {updated} updated · {skipped} missing candidates",
    ),
    ("notice.tag.candidate_accepted", "Accepted candidate #{tag}"),
    ("notice.tag.candidate_ignored", "Ignored candidate #{tag}"),
    ("notice.tag.candidate_pending", "Candidate #{tag} reopened"),
    ("notice.tag.batch_reviewed", "Reviewed {changed} candidates"),
    (
        "notice.tag.retag",
        "Retagging · {recomputed} recomputed · {queued} queued",
    ),
    (
        "notice.tag_queue.paused",
        "Tagging paused · current batch finishes, no new jobs claimed",
    ),
    (
        "notice.tag_queue.cancelling",
        "Cancelling tagging · completed results kept",
    ),
    ("notice.tag_queue.running", "Tagging already running"),
    ("notice.tag_queue.resuming", "Resuming tagging..."),
    (
        "notice.tag_queue.failed_paused",
        "Tagging failed and paused",
    ),
    (
        "notice.tag_queue.cpu_allowed",
        "Slow CPU tagging allowed; takes effect when the queue resumes",
    ),
    (
        "notice.tag_queue.gpu_paused",
        "Tagging pauses by default when GPU is unavailable",
    ),
    (
        "notice.tag_queue.loading_model",
        "Loading PixAI model... ({pending} queued)",
    ),
    (
        "notice.tag_queue.started",
        "Tagging 0/{total} · {classes} tag classes",
    ),
    (
        "notice.tag_queue.gpu_unavailable_scan",
        "GPU unavailable ({device}) · tagging paused, {pending} queued; allow slow CPU tagging to continue",
    ),
    (
        "notice.tag_queue.gpu_unavailable_worker",
        "GPU unavailable ({device}) · tagging paused; allow slow CPU tagging to continue",
    ),
    (
        "notice.tag_queue.progress",
        "Tagging {done}/{total} · {pending} queued · {failed} failed",
    ),
    (
        "notice.tag_queue.cancelled",
        "Cancelled · {done}/{total} done · {pending} pending",
    ),
    (
        "notice.tag_queue.paused_queue",
        "Tagging paused · {pending} pending · {failed} failed",
    ),
    (
        "notice.tag_queue.paused_pending",
        "Tagging paused · {pending} pending · {failed} failed",
    ),
    ("notice.tag_queue.complete", "Done · {total} images tagged"),
    ("notice.tag_queue.no_sources", "No sources to tag"),
    ("notice.tag_queue.no_jobs", "No tagging jobs pending"),
    ("notice.cleanup.kept", "Kept {n} missing-image records"),
    (
        "notice.cleanup.removed",
        "Removed {removed} missing-image records",
    ),
    (
        "notice.cleanup.removed_kept",
        "Removed {removed} missing records; kept {kept} recovered files",
    ),
    ("notice.repair.running", "Repairing · {label}"),
    ("notice.repair.failed", "Repair failed · {label}"),
    ("notice.repair.failed_plain", "Repair failed"),
    (
        "notice.repair.cache_rebuilt_loading",
        "Preview cache rebuilt · loading tagging model...",
    ),
    (
        "notice.repair.cache_rebuilt_incomplete",
        "Preview rebuilt, tagging incomplete",
    ),
    (
        "notice.repair.cache_rebuilt_retag",
        "Preview rebuilt · retagging...",
    ),
    (
        "notice.repair.done_damage",
        "Repair complete ({damage}) · {name}",
    ),
    ("notice.repair.done", "Repair complete · {name} is readable"),
    (
        "notice.repair.incomplete",
        "Repair incomplete · file still not fully readable",
    ),
    (
        "notice.repair.exporting",
        "Exporting repaired copy · {label}",
    ),
    (
        "notice.repair.exported_damage",
        "Exported repaired copy ({damage}) · {path}",
    ),
    ("notice.repair.exported", "Exported repaired copy · {path}"),
    (
        "notice.repair.export_failed",
        "Failed to export repaired copy",
    ),
    (
        "notice.dedup.preselected",
        "Preselected {selected} groups by rule",
    ),
    ("notice.dedup.marked_ignored", "Marked as not duplicates"),
    ("notice.dedup.marked_related", "Marked as related variants"),
    ("notice.dedup.marked_done", "Kept the whole group"),
    ("notice.dedup.marked_reopened", "Group reopened"),
    (
        "notice.dedup.similar_too_few",
        "Need at least 2 images to scan for similar images",
    ),
    (
        "notice.dedup.similar_scanning",
        "Scanning similar images · Hamming distance ≤{threshold} · {n} images",
    ),
    (
        "notice.dedup.similar_stale",
        "Library changed; rescan similar images",
    ),
    (
        "notice.dedup.similar_done",
        "Similar scan complete · {groups} groups · {pending} unreviewed",
    ),
    ("notice.dedup.moved", "Moved {moved} to recycle bin"),
    (
        "notice.dedup.moved_partial",
        "Moved {moved} to recycle bin; {failed} still pending",
    ),
    (
        "notice.dedup.protected_skipped",
        "Skipped {skipped} protected groups; no copies could be moved",
    ),
    (
        "notice.dedup.batch_done",
        "Batch complete: {groups} groups · {assets} moved to recycle bin",
    ),
    (
        "notice.dedup.batch_done_skipped",
        "Batch complete: {groups} groups · {assets} moved to recycle bin; {skipped} protected groups skipped",
    ),
    (
        "notice.dedup.protected_added_path",
        "Protected path added: {path}",
    ),
    (
        "notice.dedup.protected_added_entry",
        "Protected entry added: {path}",
    ),
    (
        "notice.dedup.protected_removed",
        "Protected path removed: {path}",
    ),
    (
        "notice.saved_search.saved",
        "Smart collection “{name}” saved",
    ),
    (
        "notice.saved_search.renamed",
        "Smart collection renamed to “{name}”",
    ),
    (
        "notice.saved_search.updated",
        "Collection “{name}” updated from current view",
    ),
    ("notice.saved_search.pinned", "Smart collection pinned"),
    ("notice.saved_search.unpinned", "Smart collection unpinned"),
    (
        "notice.saved_search.reordered",
        "Smart collections reordered",
    ),
    ("notice.saved_search.deleted", "Smart collection deleted"),
    ("notice.album.created", "Album “{name}” created"),
    ("notice.album.renamed", "Album renamed to “{name}”"),
    ("notice.album.deleted", "Album deleted (images kept)"),
    ("notice.album.added", "Added {added} images to album"),
    (
        "notice.album.removed",
        "Removed {removed} images from album",
    ),
    ("notice.remote.removed", "Source “{name}” removed"),
    (
        "notice.remote.credentials_saved",
        "{name} credentials saved (system keychain)",
    ),
    (
        "notice.remote.credentials_cleared",
        "{name} credentials cleared",
    ),
    (
        "notice.remote.sync_done",
        "{name} sync complete · added {imported} · updated {updated} · skipped {skipped} · thumbnails {thumbs}",
    ),
    ("notice.remote.sync_failed", "{name} sync failed: {message}"),
    ("notice.remote.syncing", "Syncing {name}..."),
    (
        "notice.remote.sync_failed_plain",
        "Remote source sync failed",
    ),
    (
        "notice.trash.records_failed",
        "Moved to recycle bin · records not deleted",
    ),
    ("notice.retry.failed", "Retry failed"),
];
