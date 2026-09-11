# Runbook — sdkwork-discovery 部署 / 升级 / 回滚（中文）

适用环境：`development|test|staging|demo|production`。所有命令默认在本机 WSL 执行，远程主机加 `--host ssh://[user@]host[:port]`。镜像参考：`registry.sdkwork.com/apps/sdkwork-discovery-standalone:0.1.0`（tag 取自 `sdkwork.app.config.json` → `release.currentVersion`）。

> ℹ️ **交付姿态**：本模块 standalone 交付为宿主包（`host-package/nginx`），cloud 交付为容器镜像（`container-image/kubernetes`）。
> 本文档 §1 安装 / §2 升级描述的 bundle 容器安装路径（`deployments/docker/bundle/`）仅在该模块启用 standalone **容器**安装时适用；
> 宿主包路径请使用 `bin/apps-package.sh` + `bin/apps-pkg-installer.sh`，cloud 路径由 kubernetes 编排消费 `bin/docker-image.sh push` 产出的镜像。

## 1. 安装（首次）

```bash
bin/docker-deploy.sh install --environment <development|test|staging|demo|production>
bin/docker-deploy.sh install --environment production --yes   # 生产必须显式 --yes
```

install 会同步 bundle 到 `/opt/deploy/sdkwork-discovery/bundle`，加载镜像，按实例启动并等待健康门禁（`/healthz`）。

## 2. 升级

staging/demo/production 自动先生成变更前备份（`--skip-backup` 可跳过，会记录证据）：

```bash
bin/docker-image.sh build
bin/docker-deploy.sh upgrade --environment staging --image-tag 0.1.0
```

## 3. 验证（发布门禁）

```bash
bin/docker-deploy.sh status --environment staging
bin/doctor.sh --environment staging          # 聚合诊断（9 项检查）
```

## 4. 回滚

```bash
bin/docker-deploy.sh rollback --environment staging                  # 台账上一个成功版本
bin/docker-deploy.sh rollback --environment staging --to 0.1.0       # 指定版本
```

回滚由管理端口 `/healthz` 门禁把关；失败自动回退并写入 `release-state/<env>/ledger.jsonl`。迁移是前向的：跨不兼容 schema 只能走数据恢复（backup-restore.md）。

## 5. 下线

```bash
bin/docker-deploy.sh down --environment staging
bin/docker-deploy.sh stop    --environment staging   # 停止（保留容器与卷，不重打包）
bin/docker-deploy.sh start   --environment staging   # 启动已停止的栈（先起嵌入式依赖）
bin/docker-deploy.sh restart --environment staging   # 只重启应用实例（依赖不中断）
bin/docker-deploy.sh down --environment staging --purge --yes
```

<!-- generated: scaffold-module-runbooks.mjs -->
