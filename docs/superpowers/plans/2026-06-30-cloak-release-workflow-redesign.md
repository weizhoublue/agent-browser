# cloak-release Workflow 重设计 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 `cloak-release.yml` 从"rebase onto upstream/main"重写为"cherry-pick onto upstream tag"，使每次发布可复现、补丁来源明确。

**Architecture:** Job 1（prepare）负责自动检测/验证上游 tag、找出 cloak 顶部连续的 weizhoublue commits、cherry-pick 到 upstream tag 上、跑单元测试、推送 cloak 和 release 分支。Job 2（build matrix）和 Job 3（publish）结构与现有基本相同，仅更新变量引用和 release notes 格式。

**Tech Stack:** GitHub Actions YAML, Bash, Rust（cargo test）, gh CLI

## Global Constraints

- pnpm 作为包管理器（本次不涉及 JS 依赖，仅 cargo）
- CLI flags 使用 kebab-case
- 不使用 emoji
- `PATCH_AUTHOR` 匹配方式：git author name 为 `weizhoublue`，email 为 `weizhou.lan@daocloud.io`，过滤时按 author name 匹配
- workflow 文件位置：`.github/workflows/cloak-release.yml`（原地替换，不新建文件）
- `actions/checkout` 保持 v6，`actions/upload-artifact` / `download-artifact` 保持 v7

---

### Task 1: 重写 `prepare` job — 输入、Tag 检测、cherry-pick

**Files:**
- Modify: `.github/workflows/cloak-release.yml`（全文替换 `prepare` job 及 `on:` / `env:` 块）

**Interfaces:**
- Produces outputs 供 Task 2/3 使用：
  - `upstream_tag`：字符串，如 `v0.29.1`
  - `release_branch`：字符串，如 `release/v0.29.1`
  - `patch_shas`：空格分隔的 SHA 列表，如 `1cd530e a9ef29d ed3629b`（oldest-first）

- [ ] **Step 1: 替换 `on:` 输入块**

将文件顶部的 `on:` 块替换为：

```yaml
on:
  workflow_dispatch:
    inputs:
      upstream_tag:
        description: "上游 tag（如 v0.29.1）。留空则自动取上游最新 tag"
        required: false
        type: string
```

- [ ] **Step 2: 更新 `env:` 块，添加 PATCH_AUTHOR**

```yaml
env:
  UPSTREAM_REPO: vercel-labs/agent-browser
  FORK_REPO: weizhoublue/agent-browser
  ENHANCEMENT_BRANCH: cloak
  PATCH_AUTHOR: weizhoublue
```

- [ ] **Step 3: 替换 `prepare` job 的 outputs 声明**

```yaml
jobs:
  prepare:
    name: Detect tag, cherry-pick patches, test, push branches
    runs-on: ubuntu-latest
    timeout-minutes: 30
    outputs:
      upstream_tag: ${{ steps.meta.outputs.upstream_tag }}
      release_branch: ${{ steps.meta.outputs.release_branch }}
      patch_shas: ${{ steps.cherry.outputs.patch_shas }}
```

- [ ] **Step 4: 写 "Checkout (full history)" step**

```yaml
    steps:
      - name: Checkout (full history, all branches)
        uses: actions/checkout@v6
        with:
          fetch-depth: 0
          token: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **Step 5: 写 "Fetch upstream + detect/validate tag" step**

```yaml
      - name: Fetch upstream and detect tag
        id: meta
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
        run: |
          set -euo pipefail
          git remote add upstream "https://github.com/${{ env.UPSTREAM_REPO }}.git" \
            || git remote set-url upstream "https://github.com/${{ env.UPSTREAM_REPO }}.git"
          git fetch upstream --tags --force

          INPUT_TAG="${{ inputs.upstream_tag }}"
          INPUT_TAG="${INPUT_TAG#"${INPUT_TAG%%[![:space:]]*}"}"
          INPUT_TAG="${INPUT_TAG%"${INPUT_TAG##*[![:space:]]}"}"

          if [ -z "$INPUT_TAG" ]; then
            # 从上游所有 tag 中取最新（按 creatordate）
            UPSTREAM_TAG=$(git tag --sort=-creatordate | grep -E '^v[0-9]' | head -1)
            if [ -z "$UPSTREAM_TAG" ]; then
              echo "::error::未能从上游获取任何 tag"
              exit 1
            fi
            echo "自动检测到上游最新 tag: ${UPSTREAM_TAG}"
          else
            UPSTREAM_TAG="$INPUT_TAG"
            # 验证 tag 在上游存在
            if ! git rev-parse "refs/tags/${UPSTREAM_TAG}" >/dev/null 2>&1; then
              echo "::error::tag ${UPSTREAM_TAG} 在上游 ${UPSTREAM_REPO} 中不存在"
              exit 1
            fi
            echo "使用指定 tag: ${UPSTREAM_TAG}"
          fi

          echo "upstream_tag=${UPSTREAM_TAG}" >> "$GITHUB_OUTPUT"
          echo "release_branch=release/${UPSTREAM_TAG}" >> "$GITHUB_OUTPUT"
          git log -1 --oneline "refs/tags/${UPSTREAM_TAG}"
```

- [ ] **Step 6: 写 "Configure git author" step**

```yaml
      - name: Configure git author
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
```

- [ ] **Step 7: 写 "Find and cherry-pick weizhoublue patches" step**

这是核心逻辑：从 `origin/cloak` 顶部收集连续的 weizhoublue commits（oldest-first），然后 cherry-pick 到 upstream tag 上。

```yaml
      - name: Find and cherry-pick fork patches onto upstream tag
        id: cherry
        run: |
          set -euo pipefail
          git fetch origin "${{ env.ENHANCEMENT_BRANCH }}"

          # 从 origin/cloak HEAD 向下遍历，收集顶部连续的 PATCH_AUTHOR commits
          PATCHES=()
          while IFS=' ' read -r SHA ANAME; do
            if [ "$ANAME" = "${{ env.PATCH_AUTHOR }}" ]; then
              PATCHES=("$SHA" "${PATCHES[@]}")   # 头插，保持 oldest-first 顺序
            else
              break
            fi
          done < <(git log "origin/${{ env.ENHANCEMENT_BRANCH }}" --format="%H %an")

          if [ ${#PATCHES[@]} -eq 0 ]; then
            echo "::error::origin/${{ env.ENHANCEMENT_BRANCH }} 顶端未找到 ${{ env.PATCH_AUTHOR }} 的 commits"
            exit 1
          fi
          echo "找到 ${#PATCHES[@]} 个补丁：${PATCHES[*]}"

          # 在 upstream tag 处新建工作分支（覆盖本地 cloak）
          git checkout -B "${{ env.ENHANCEMENT_BRANCH }}" "refs/tags/${{ steps.meta.outputs.upstream_tag }}"

          # 按 oldest-first 顺序逐个 cherry-pick
          for SHA in "${PATCHES[@]}"; do
            echo "cherry-pick: $SHA"
            if ! git cherry-pick "$SHA"; then
              echo "::error::cherry-pick $SHA 失败，请本地解决冲突后重新触发"
              git cherry-pick --abort || true
              exit 1
            fi
          done

          # 恢复 fork 自身的 CI workflows，防止被上游内容覆盖
          git checkout "origin/${{ env.ENHANCEMENT_BRANCH }}" -- .github/workflows/
          git commit -m "Restore fork workflows after cherry-pick" || true

          # 输出 patch SHA 列表（oldest-first，空格分隔）供 release notes 使用
          PATCH_SHAS="${PATCHES[*]}"
          echo "patch_shas=${PATCH_SHAS}" >> "$GITHUB_OUTPUT"
```

- [ ] **Step 8: 写 "Run unit tests" step**

```yaml
      - name: Run unit tests
        run: |
          set -euo pipefail
          cd cli
          cargo test
```

- [ ] **Step 9: 写 "Push cloak and release branch" step**

```yaml
      - name: Push cloak branch and release archive
        run: |
          set -euo pipefail
          # 覆盖更新 cloak（cherry-pick 后的最终状态）
          git push origin HEAD:"${{ env.ENHANCEMENT_BRANCH }}" --force
          echo "Updated origin/${{ env.ENHANCEMENT_BRANCH }}"

          # 新建存档分支
          BRANCH="${{ steps.meta.outputs.release_branch }}"
          git push origin HEAD:"$BRANCH" --force
          echo "Archived source snapshot at origin/${BRANCH}"
```

- [ ] **Step 10: 本地验证 YAML 语法**

```bash
cd /Users/weizhoulan/Documents/forkgit/agent-browser
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/cloak-release.yml'))" && echo "YAML OK"
```

期望输出：`YAML OK`

- [ ] **Step 11: Commit**

```bash
git add .github/workflows/cloak-release.yml
git commit -m "ci: rewrite prepare job to cherry-pick onto upstream tag"
```

---

### Task 2: 更新 `build-binaries` job — 引用新输出变量

**Files:**
- Modify: `.github/workflows/cloak-release.yml`（`build-binaries` job 内 outputs 引用）

**Interfaces:**
- Consumes: `needs.prepare.outputs.upstream_tag`（替换旧的 `release_tag`）

- [ ] **Step 1: 更新 cache key 引用**

将 `build-binaries` job 中的：

```yaml
          shared-key: cloak-release-${{ needs.prepare.outputs.release_tag }}
```

替换为：

```yaml
          shared-key: cloak-release-${{ needs.prepare.outputs.upstream_tag }}
```

- [ ] **Step 2: 验证 YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/cloak-release.yml'))" && echo "YAML OK"
```

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/cloak-release.yml
git commit -m "ci: update build job to use upstream_tag output"
```

---

### Task 3: 更新 `publish-release` job — release notes 加入溯源信息

**Files:**
- Modify: `.github/workflows/cloak-release.yml`（`publish-release` job）

**Interfaces:**
- Consumes:
  - `needs.prepare.outputs.upstream_tag`
  - `needs.prepare.outputs.release_branch`
  - `needs.prepare.outputs.patch_shas`（空格分隔 SHA 列表）

- [ ] **Step 1: 更新 "Write release notes" step 的 env 块**

将旧的 `TAG: ${{ needs.prepare.outputs.release_tag }}` 替换为：

```yaml
        env:
          TAG: ${{ needs.prepare.outputs.upstream_tag }}
          BRANCH: ${{ needs.prepare.outputs.release_branch }}
          PATCH_SHAS: ${{ needs.prepare.outputs.patch_shas }}
          UPSTREAM_REPO: ${{ env.UPSTREAM_REPO }}
          FORK_REPO: ${{ env.FORK_REPO }}
          ENHANCEMENT_BRANCH: ${{ env.ENHANCEMENT_BRANCH }}
```

- [ ] **Step 2: 重写 release notes 生成脚本**

在同一 step 的 `run:` 块中替换为：

```yaml
        run: |
          set -euo pipefail
          {
            echo "## agent-browser ${TAG} (fork)"
            echo
            echo "- **Upstream base**: [\`${UPSTREAM_REPO}\`](https://github.com/${UPSTREAM_REPO}) tag \`${TAG}\`"
            echo "- **Fork patches** cherry-picked from \`${ENHANCEMENT_BRANCH}\`:"
            for SHA in $PATCH_SHAS; do
              MSG=$(git log -1 --format="%s" "$SHA" 2>/dev/null || echo "(unknown)")
              echo "  - \`${SHA:0:7}\` ${MSG}"
            done
            echo "- **Archive branch**: \`${BRANCH}\`"
            echo "- **Binaries**: \`agent-browser\` (platform-specific release filenames)"
            echo
            echo "### Assets"
            echo
            echo "| File | Platform |"
            echo "|------|----------|"
            echo "| \`agent-browser-linux-amd64\` | Linux x86_64 |"
            echo "| \`agent-browser-linux-arm64\` | Linux ARM64 |"
            echo "| \`agent-browser-darwin-arm64\` | macOS Apple Silicon |"
            echo
            echo "Verify checksums: \`sha256sum -c SHA256SUMS\`"
            echo
            echo "Usage: [get-started.md](https://github.com/${FORK_REPO}/blob/${BRANCH}/get-started.md)"
          } > /tmp/release-notes.md
          cat /tmp/release-notes.md
```

- [ ] **Step 3: 更新 "Create or update GitHub Release" step**

将 `TAG="${{ needs.prepare.outputs.release_tag }}"` 替换为：

```yaml
          TAG="${{ needs.prepare.outputs.upstream_tag }}"
```

- [ ] **Step 4: 删除 "Push archival branch release/<tag>" step**

该 step 已移入 Task 1 的 prepare job，在 `publish-release` job 中删除以下内容：

```yaml
      - name: Configure git author
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"

      - name: Push archival branch release/<tag>
        run: |
          set -euo pipefail
          BRANCH="${{ needs.prepare.outputs.release_branch }}"
          git checkout -B "$BRANCH"
          git push origin "$BRANCH" --force
          echo "Archived source snapshot at origin/${BRANCH}"
```

- [ ] **Step 5: 验证完整 YAML**

```bash
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/cloak-release.yml'))" && echo "YAML OK"
```

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/cloak-release.yml
git commit -m "ci: update publish job with upstream tag and patch SHA provenance"
```

---

### Task 4: 端到端 Dry-run 验证

**Files:**
- Read: `.github/workflows/cloak-release.yml`（最终全文核查）

- [ ] **Step 1: 确认 job 依赖链完整**

```bash
grep -E "(needs:|outputs:|steps\.\w+\.outputs\.)" .github/workflows/cloak-release.yml
```

期望看到：
- `build-binaries` 的 `needs: prepare`
- `publish-release` 的 `needs: [prepare, build-binaries]`
- 所有 `needs.prepare.outputs.release_tag` 均已替换为 `upstream_tag`

- [ ] **Step 2: 在本地模拟 cherry-pick 脚本**

在仓库根目录运行（只读，不推送），验证补丁检测逻辑：

```bash
cd /Users/weizhoulan/Documents/forkgit/agent-browser
PATCH_AUTHOR="weizhoublue"
BRANCH="cloak"
PATCHES=()
while IFS=' ' read -r SHA ANAME; do
  if [ "$ANAME" = "$PATCH_AUTHOR" ]; then
    PATCHES=("$SHA" "${PATCHES[@]}")
  else
    break
  fi
done < <(git log "origin/$BRANCH" --format="%H %an")
echo "检测到 ${#PATCHES[@]} 个补丁（oldest-first）:"
for SHA in "${PATCHES[@]}"; do
  git log -1 --oneline "$SHA"
done
```

期望输出：3 个 commit（`1cd530e`、`a9ef29d`、`ed3629b`），oldest-first 顺序。

- [ ] **Step 3: 确认 release notes 脚本中 `$SHA:0:7` 语法可在 runner 上运行**

GitHub Actions runner 使用 `bash`，`${SHA:0:7}` 是标准 bash 字符串截取，无需额外处理。但 `run:` 中嵌套的 `for SHA in $PATCH_SHAS` 依赖 word splitting，`PATCH_SHAS` 是空格分隔字符串，需确保不加引号：

检查 Task 3 Step 2 中的 `for SHA in $PATCH_SHAS` 一行未加引号。

- [ ] **Step 4: 最终 commit（若 Task 4 有任何修正）**

```bash
git add .github/workflows/cloak-release.yml
git commit -m "ci: final dry-run fixes for cloak-release workflow"
```

若无修正则跳过此步。

---

## 自检（Self-Review）

**Spec coverage 核查：**

| Spec 要求 | 对应 Task |
|---|---|
| 输入可选，留空自动取上游最新 tag | Task 1 Step 5 |
| 验证 tag 存在于上游 | Task 1 Step 5 |
| cherry-pick 顶部连续 weizhoublue commits | Task 1 Step 7 |
| .github/workflows/ 保护 | Task 1 Step 7（cherry-pick 后 git checkout）|
| cherry-pick 失败报错 | Task 1 Step 7（git cherry-pick --abort + exit 1）|
| 单元测试门控 | Task 1 Step 8 |
| force push cloak | Task 1 Step 9 |
| 新建 release/<tag> 存档 | Task 1 Step 9 |
| release notes 含上游 tag 和补丁 SHA | Task 3 Step 2 |
| 删除旧的 push archival branch step | Task 3 Step 4 |
| main 分支不被修改 | 整个 workflow 无 push to main 操作 |
