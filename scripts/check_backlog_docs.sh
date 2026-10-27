#!/bin/bash
# Backlog 一致性校验：ID 完整性 + 状态互斥
set -u
cd /d/Code/Kanalib/docs/backlog || exit 1

reg() { grep -E "^\| F-[0-9]{3} " registry.md | awk -F'|' '{gsub(/^ +| +$/,"",$2); gsub(/^ +| +$/,"",$5); print $2"|"$5}'; }

echo "== registry 条目数 / 去重数 =="
reg | cut -d'|' -f1 | sort > /tmp/r1
reg | cut -d'|' -f1 | sort -u > /tmp/r2
echo "rows=$(wc -l < /tmp/r1) unique=$(wc -l < /tmp/r2)"
echo
echo "== 状态分布 =="
reg | cut -d'|' -f2 | sort | uniq -c | sort -rn
echo
echo "== registry 标 已实现，但 implemented.md 无归档行 =="
reg | grep '|已实现' | cut -d'|' -f1 | sort > /tmp/impl_reg
grep -E "^\| F-[0-9]{3} " implemented.md | awk -F'|' '{gsub(/^ +| +$/,"",$2); print $2}' | sort -u > /tmp/impl_doc
comm -23 /tmp/impl_reg /tmp/impl_doc
echo
echo "== implemented.md 有行，但 registry 状态不是 已实现 =="
while read -r id; do
  st=$(reg | grep "^$id|" | cut -d'|' -f2)
  [ -z "$st" ] && st="NOT-IN-REGISTRY"
  case "$st" in
    已实现) ;;
    *) echo "$id -> $st" ;;
  esac
done < /tmp/impl_doc
echo
echo "== pending/overview 里的 ID 与 registry 状态 =="
grep -E "^\| F-[0-9]{3} " pending/overview.md | awk -F'|' '{gsub(/^ +| +$/,"",$2); print $2}' | sort -u > /tmp/pend
while read -r id; do
  st=$(reg | grep "^$id|" | cut -d'|' -f2)
  echo "$id ${st:-NOT-IN-REGISTRY}"
done < /tmp/pend
echo
echo "== overview 未列出、但 registry 处于 待确认/实现中 的 ID =="
reg | grep -E '\|(待确认|实现中|待实现)\|?$' | awk -F'|' '{print $1}' | sort > /tmp/undone
comm -13 /tmp/pend /tmp/undone
echo
echo "== pending 详情文件是否覆盖 overview 的 ID =="
for id in $(cat /tmp/pend); do
  if ! grep -rq "$id" pending/browse.md pending/dedup.md pending/library-sources-formats.md pending/ops.md pending/search-organization.md pending/tags.md; then
    echo "$id 没有出现在任何 pending 详情文件"
  fi
done
echo "== 校验结束 =="
