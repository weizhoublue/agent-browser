# cloak-release workflow 重设计

**日期**: 2026-06-30
**状态**: 已批准，待实现
**文件**: `.github/workflows/cloak-release.yml`

## 背景与问题

现有 workflow 存在以下可靠性隐患：

1. `release_tag` 强制手动输入，格式为自造的 `v0.x.x-cloak.1`，与上游 tag 解耦，难以追溯
2. rebase 策略以 `upstream/main` HEAD 为基点，每次运行基点可能不同，结果不可复现
3. `git push --force origin HEAD:main` 强制覆盖 fork 的 main 分支，风险高
4. 补丁范围模糊，整个 `cloak` 分支都参与 rebase，无法区分上游 commits 和 fork 补丁

## 目标

- 以上游官方 tag 为固定基点，结果可复现
- 自动检测最新上游 tag，输入可选
- 只摘取明确属于 fork 的补丁（顶部连续 weizhoublue commits）
- 测试通过后再落地分支，不动 fork main

## 架构

### Job 依赖图

```
prepare (tag检测 → cherry-pick → 单元测试 → push branches)
    │
    ├──→ build-linux-amd64  ─┐
    ├──→ build-linux-arm64  ─┤──→ publish-release
    └──→ build-darwin-arm64 ─┘
```

### 环境变量

```yaml
env:
  UPSTREAM_REPO: vercel-labs/agent-browser
  FORK_REPO: weizhoublue/agent-browser
  ENHANCEMENT_BRANCH: cloak
  PATCH_AUTHOR: weizhoublue
```

## 详细设计

### 第一节：输入与 Tag 检测

**workflow 输入**

```yaml
on:
  workflow_dispatch:
    inputs:
      upstream_tag:
        description: "上游 tag（如 v0.29.1）。留空则自动取上游最新 tag"
        required: false
        type: string
```

`upstream_tag` 替换旧的 `release_tag`，语义明确：这是上游仓库的真实 tag。

**自动检测逻辑**

```bash
if [ -z "$INPUT_TAG" ]; then
  # 从上游获取最新 tag（按 creatordate 取最后一个）
  UPSTREAM_TAG=$(gh api repos/vercel-labs/agent-browser/git/refs/tags \
    --jq '.[-1].ref | ltrimstr("refs/tags/")')
else
  UPSTREAM_TAG="$INPUT_TAG"
  # 验证该 tag 确实存在于上游，不存在则报错退出
  gh api repos/vercel-labs/agent-browser/git/ref/tags/$UPSTREAM_TAG
fi
```

**release branch 命名**

`release/<upstream_tag>`，例如 `release/v0.29.1`。不再需要维护 `-cloak.1` 后缀。

### 第二节：Cherry-pick 逻辑

**找出顶部连续 weizhoublue commits**

从 `origin/cloak` HEAD 向下遍历，收集连续的 weizhoublue commits（按邮箱匹配），遇到第一个非 weizhoublue commit 停止。结果以 oldest-first 顺序存入数组供 cherry-pick 使用。

```bash
PATCHES=()
while IFS= read -r line; do
  SHA=$(echo "$line" | cut -d' ' -f1)
  EMAIL=$(echo "$line" | cut -d' ' -f2)
  if [[ "$EMAIL" == *"weizhoublue"* ]]; then
    PATCHES=("$SHA" "${PATCHES[@]}")   # 头插，保持 oldest-first 顺序
  else
    break
  fi
done < <(git log origin/cloak --format="%H %ae")

if [ ${#PATCHES[@]} -eq 0 ]; then
  echo "::error::origin/cloak 顶端未找到 weizhoublue 的 commits"
  exit 1
fi
```

**cherry-pick 到上游 tag**

```bash
# 在上游 tag 处新建工作分支
git checkout -B cloak "refs/tags/$UPSTREAM_TAG"

# 按 oldest-first 顺序逐个 cherry-pick
for SHA in "${PATCHES[@]}"; do
  if ! git cherry-pick "$SHA"; then
    echo "::error::cherry-pick $SHA 失败，请本地解决冲突后重新触发"
    git cherry-pick --abort || true
    exit 1
  fi
done
```

**workflows 目录保护**

cherry-pick 完成后，恢复 fork 自身的 CI 文件，防止被上游 tag 内容覆盖：

```bash
git checkout origin/cloak -- .github/workflows/
git commit -m "Restore fork workflows after cherry-pick" || true
```

**新旧方式对比**

| 维度 | 旧方式（rebase cloak onto main） | 新方式（cherry-pick onto tag） |
|---|---|---|
| 基点 | upstream/main HEAD（随时变动） | 固定的 upstream tag（可复现） |
| 补丁识别 | 整个 cloak 分支 | 仅顶部连续 weizhoublue commits |
| 失败后恢复 | 手动 rebase --abort，再本地处理 | cherry-pick --abort，SHA 明确可定位 |
| 可复现性 | 同一 tag 多次运行结果可能不同 | 确定性：tag + patch SHAs 固定 |

### 第三节：测试 + 分支推送

**单元测试**

cherry-pick 成功后，在推送任何分支之前运行：

```bash
cd cli && cargo test
```

测试失败则 job 立即报错，不推送任何分支，不触发构建。

**分支推送顺序**

```bash
# 1. 覆盖更新 cloak（cherry-pick 后的最终状态）
git push origin HEAD:cloak --force

# 2. 新建存档分支
git push origin HEAD:release/$UPSTREAM_TAG
```

两次推送指向同一 commit，`release/<version>` 是 `cloak` 当时状态的快照，供日后回溯。fork 的 `main` 分支不参与，不再被覆盖。

### 第四节：构建矩阵 + 发布

**构建（Job 2）**

与现有逻辑基本不变，从 `cloak` 分支 checkout，matrix 并行构建三平台：

- `agent-browser-linux-amd64`（x86_64, zigbuild）
- `agent-browser-linux-arm64`（aarch64, zigbuild）
- `agent-browser-darwin-arm64`（Apple Silicon, cargo）

**Release Notes（Job 3）**

新增溯源信息，记录上游 tag 和 cherry-pick 的补丁 SHA：

```
## agent-browser v0.29.1 (fork)

- Upstream base: vercel-labs/agent-browser v0.29.1
- Fork patches cherry-picked from cloak:
  - 1cd530e cloak
  - a9ef29d 解决 cdp 竞争问题
  - ed3629b auto commit: 2026-06-22 17:46:21
- Archive branch: release/v0.29.1
```

## 变更总结

| 旧流程 | 新流程 |
|---|---|
| 必须手动输入自造版本号 | 输入可选，留空自动取上游最新 tag |
| rebase cloak onto upstream/main（HEAD 随时变） | cherry-pick onto 固定 upstream tag |
| force push upstream/main 到 fork main | 不动 main，只更新 cloak |
| 版本号和上游 tag 解耦 | release 直接对应上游 tag |
| 补丁范围模糊（整个 cloak 分支） | 仅顶部连续 weizhoublue commits，清晰可控 |
| 无测试门控 | cherry-pick 后先跑单元测试再推送 |

## 实现说明

- 不影响 `cloak-build.yml`（手动构建任意 ref 的 workflow）
- `cloak` 分支在每次发布后被 force push 覆盖，是"当前发布状态"的指针
- `origin/cloak` 在 cherry-pick 之前始终是"补丁来源"，不在 CI 中被直接修改
