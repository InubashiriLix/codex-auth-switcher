#!/bin/bash
# 测试守护进程功能

set -e

echo "=== Codex Switcher 守护进程测试 ==="
echo

# 颜色
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# 构建
echo -e "${YELLOW}1. 编译项目...${NC}"
cargo build --release
echo -e "${GREEN}✓ 编译完成${NC}"
echo

# 检查状态
echo -e "${YELLOW}2. 检查守护进程状态...${NC}"
./target/release/codex-switcher daemon-status || true
echo

# 启动守护进程（后台）
echo -e "${YELLOW}3. 启动守护进程...${NC}"
RUST_LOG=info ./target/release/codex-switcher --daemon &
DAEMON_PID=$!
echo -e "${GREEN}✓ 守护进程已启动 (PID: $DAEMON_PID)${NC}"
echo

# 等待启动
sleep 2

# 再次检查状态
echo -e "${YELLOW}4. 验证守护进程状态...${NC}"
./target/release/codex-switcher daemon-status
echo

# 测试代理连接
echo -e "${YELLOW}5. 测试代理连接...${NC}"
if command -v curl &> /dev/null; then
    curl -s -o /dev/null -w "%{http_code}" \
        -x http://127.0.0.1:8765 \
        --connect-timeout 5 \
        https://httpbin.org/get || echo -e "${YELLOW}(代理测试失败 - 这是正常的，因为没有配置token)${NC}"
else
    echo -e "${YELLOW}(未安装curl，跳过代理测试)${NC}"
fi
echo

# 测试热重载
echo -e "${YELLOW}6. 测试热重载...${NC}"
sleep 1
./target/release/codex-switcher daemon-reload
echo -e "${GREEN}✓ 热重载信号已发送${NC}"
echo

# 等待处理
sleep 2

# 停止守护进程
echo -e "${YELLOW}7. 停止守护进程...${NC}"
./target/release/codex-switcher daemon-stop
sleep 2
echo -e "${GREEN}✓ 守护进程已停止${NC}"
echo

# 验证停止
echo -e "${YELLOW}8. 验证守护进程已停止...${NC}"
./target/release/codex-switcher daemon-status || true
echo

echo -e "${GREEN}=== 所有测试完成 ===${NC}"
echo
echo "提示："
echo "- 使用 'codex-switcher --daemon' 启动守护进程"
echo "- 使用 'codex-switcher daemon-status' 检查状态"
echo "- 使用 'codex-switcher daemon-reload' 热重载配置"
echo "- 使用 'codex-switcher daemon-stop' 停止守护进程"
echo
echo "配置 Codex CLI:"
echo "  export HTTPS_PROXY=http://127.0.0.1:8765"
