#!/bin/bash
# 测试./run.sh
# nc 192.168.0.2 80 使用nc发起TCP连接
# 
# 停止 pkill -f run.sh
#
# 只编译不运行
cargo build --release

ext=$?           # 保存上一个命令的退出码
echo "Exit code: $ext"

if [[ $ext != 0 ]]; then
    exit $ext    # 如果失败，以相同状态码退出
fi

# 只需第一次运行时执行，后续可注释
sudo setcap cap_net_admin=+ep target/release/tcp-rust

# 使用 `ip` 命令删除整个设备
if ip link show tun0 > /dev/null 2>&1; then
  echo "tun0 存在，正在删除..."
  sudo ip link delete tun0
else
  echo "tun0 不存在，无需删除"
fi

# 启动程序放后台
./target/release/tcp-rust &
# $!最近一个后台执行的命令的 进程 ID（PID）。
# 把最近启动的后台进程的 PID 存入变量 pid。
pid=$!


# 禁用特定接口上的 IPv6 支持，阻止该接口收发 IPv6 数据包
# 等待 tun0 创建（可适当延迟或轮询）
echo "等待 tun0 设备创建..."
while ! ip link show tun0 > /dev/null 2>&1; do
  sleep 0.1
done

sudo sysctl -w net.ipv6.conf.tun0.disable_ipv6=1



# 给 tun0 配置 IP
sudo ip addr add 192.168.0.1/24 dev tun0
sudo ip link set dev tun0 up

# 捕获 TERM 信号，终止后台进程
trap "kill $pid" TERM

# 脚本运行完毕，后台程序继续执行
echo ""
echo "程序已后台启动，tun0 配置完成"
echo "fg放到前台执行 bg放到后台执行"
# 等待后台程序结束
wait $pid