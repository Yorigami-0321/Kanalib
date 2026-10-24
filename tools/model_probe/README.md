# PixAI Tagger v1.0 可行性探针

本目录用于回答一个问题：**PixAI Tagger v1.0 能否自动给本地图片打标签/分类，以及边界在哪。**

已验证：CPU（Python 3.10 + torch 2.9.1 CPU）和 GPU（RTX 4060 Laptop 8GB + torch 2.9.1+cu130）都能完整加载模型并输出高质量动漫标签；FP16 约 3.9 张/秒。

## 目录

```text
probe.py                 单/多图推理，输出分类标签、置信度、耗时、显存
fetch_torch_cuda.py      SJTU CUDA torch 轮子分段下载（2.86GB，支持断点续跑）
fixtures/                本地测试图（动漫少女、Q 版、UI、实景）
out*/                    probe.json 与日志
```

模型与依赖体积较大，均不应提交到版本库。

## 1. 环境（uv + 国内镜像）

```bash
cd tools/model_probe
uv venv .venv-cpu --python 3.10

uv pip install --python .venv-cpu/Scripts/python.exe \
  --index-url https://pypi.tuna.tsinghua.edu.cn/simple \
  torch==2.9.1 torchvision==0.24.1 transformers==4.57.6 \
  timm pillow numpy safetensors
```

PyTorch CUDA 12.8 轮子（可选，GPU 用）：

- 索引：`https://mirror.sjtu.edu.cn/pytorch-wheels/cu128`（也测过阿里云同路径，可用）
- cp310：`torch-2.9.1+cu128-cp310-cp310-win_amd64.whl`（2.86GB）
- 配套：`torchvision-0.24.1+cu128-cp310-cp310-win_amd64.whl`

SJTU 单连接约 0.5–1.2MB/s，`fetch_torch_cuda.py` 用 16 路 Range 分段；torchvision 小文件建议 `curl -C -` 断点续传。安装时用本地 wheel + 国内 PyPI 补依赖，避免 uv 单线程拉 2.86GB。

## 2. 模型（ModelScope，国内）

HuggingFace 直连/镜像的大文件走 Xet，实测约 150KB/s；ModelScope 的 `repo?FilePath=` 直文件流约 2–4MB/s，且校验和与 HF 一致。

```bash
mkdir -p models/pixai-tagger-v1.0
cd models/pixai-tagger-v1.0

base="https://www.modelscope.cn/api/v1/models/pixai-labs/pixai-tagger-v1.0/repo?Revision=master&FilePath="
for f in config.json preprocessor_config.json tagger_pipeline.py; do
  curl -L --fail -o "$f" "${base}${f}"
done
curl -L --fail -o model.safetensors "${base}model.safetensors"
```

校验和：

```text
model.safetensors          f29e475205cbcbc25b52a075840c7809d6215be15f45375113ba09c49bf90292
config.json                f8a19b38661c37dc5fd519f2137be70f6f897972021b4ebc17b7d35a0e27b9dd
preprocessor_config.json   20451575e627950993de7b00641d5b263eceb3e7436e4ad518fea2411b60e49d
tagger_pipeline.py         3431279fdfff234f62664999fc1fe6bb20f1e8756f7435d0e165ebd4456f7fcd
```

## 3. 运行

```bash
# 两张动漫图
.venv-cpu/Scripts/python.exe probe.py \
  fixtures/anime_girl.png fixtures/anime_chibi.png \
  --device cpu --batch-size 1 --topk 20

# 完整截图 / 纯 UI / 实景
.venv-cpu/Scripts/python.exe probe.py \
  "D:/Code/floral-notepaper/Docs/images/AI绘画截图.png" \
  "D:/Code/floral-notepaper/Docs/images/主窗口截图.png" \
  fixtures/real_img50.jpg \
  --device cpu --batch-size 1 --out out_domain

# GPU：本次直接复用本机 ComfyUI 的 CUDA 环境（仅验证用，产品必须独立 venv）
"D:/APP/ComfyUI-aki-v3/python/python.exe" probe.py \
  fixtures/anime_girl.png fixtures/anime_chibi.png \
  "D:/Code/floral-notepaper/Docs/images/AI绘画截图.png" fixtures/real_img50.jpg \
  --device cuda --batch-size 1 --bench --half --dump-logits --out out_gpu
```

`--dump-logits` 会保存 `logits.npz` 与 `input_batch.npy`，供后续 ONNX/其他实现做数值对齐。

## 4. 实测结果（CPU，FP32）

| 图片 | 关键标签 |
|---|---|
| 天使少女 | `1girl .998`, `wings .993`, `halo .990`, `solo .988`, `white_dress .937`, `head_wreath .902` |
| 猫耳 Q 版 | `animal_ears .997`, `cat_ears .989`, `heart .986`, `ahoge .984`, `heart_hands .983` |
| 完整 AI 绘图截图 | `1girl .996`, `dress .987`, `wings .978`, `halo .971`, `solo .962` |
| 纯 UI 截图 | `text_focus .982`, `no_humans .965`, `text-only_page .920`, `fake_screenshot .351` |
| 实景拼图 | `sky .884`, `cloud .801`, `outdoors .766`, `mountain .689`, `photo_(medium) .769`, `off-topic .630` |

CPU 单张耗时 **21.6–24.3 秒**（16 线程，batch=1）。模型加载约 1–2.5 秒（磁盘热）。这验证了“能分类”，也说明 CPU 不适合批量；产品必须走 CUDA。

GPU（RTX 4060 Laptop 8GB，4 张图）：

| 模式 | 吞吐 | 单张 | 峰值显存 | 标签 |
|---|---:|---:|---:|---|
| FP32 | 1.09–1.18 张/秒 | 848–916ms | 2.1–2.9GB | 基准 |
| FP16 autocast | 3.24–3.44 张/秒 | 291–308ms | 2.9–3.5GB | 基本一致 |
| FP16 权重 `--half` | **3.84–3.89 张/秒** | **257–261ms** | **1.1–1.6GB** | 0.001 级差异 |

按 FP16 3.9 张/秒，10 万张约 7.2 小时机时。批量从 1 增到 4 吞吐基本不变（compute-bound），所以不必盲目加大 batch。

## 5. 结论

- 动漫插画：**可以**，标签细、置信度高，可直接用于标签面/智能集合。
- 带 UI 的 AI 绘图截图：**可以**，主体识别不受 UI 边框明显影响。
- 纯 UI/实景：**只能取低层标签**，模型会给出 `text_focus`、`photo_(medium)`、`off-topic` 等提示；不要把它当通用照片分类器或安全审核器。
- 输出是多标签，不是单标签；产品层用 `rating / copyright+character / general+style+meta` 三层做分类与筛选。
- 本次样本都是原创角色，`character` 为空；**已知角色/作品识别尚未验证**，需要拿库里的 20–50 张已知角色图抽检 `character/copyright`。
- FP32 / FP16 autocast / FP16 权重三档的 top-20 标签集合完全一致，共有标签最大分数差 0.0013 / 0.0019。
