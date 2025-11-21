from mininet.net import Mininet
from mininet.link import TCLink
import time
import os
import multiprocessing
import tomli
from common import *

USER_NAME = "xxxx"  # your username on the system to run mahimahi

WAIT_MM_INIT = 3  # seconds. Wait for mahimahi to initialize.


class PEMINetwork:
    def __init__(
        self,
        delay1,
        delay2,
        loss1ge,
        loss1,
        loss2,
        bw1,
        bw2,
        qdisc,
        mm_config,
        loss_seed=None,
    ):
        if mm_config is not None:
            os.system(
                "sudo sysctl -w net.ipv4.ip_forward=1"
            )  # need forward for mahimahi

        self.net = Mininet(controller=None, link=TCLink)

        # Add hosts and switches
        self.h1 = self.net.addHost("h1", ip=ip(1), mac=mac(1))  # client
        self.h2 = self.net.addHost("h2", ip=ip(2), mac=mac(2))  # server
        self.r1 = self.net.addHost("r1")

        # Add links
        self.net.addLink(self.r1, self.h1)
        self.net.addLink(self.r1, self.h2)
        self.net.build()

        # Calculate the BDP
        # https://unix.stackexchange.com/questions/100785/bucket-size-in-tbf
        rtt_ms = 2 * (delay1 + delay2)
        bw_mbps = min(bw1, bw2)
        bdp = get_max_queue_size_bytes(rtt_ms, bw_mbps)
        pemilog(f"max_queue_size (bytes) = {bdp}")
        bdp_mm_pkts = max(1, int(bdp / 1500))  # bdp/pkt_size

        # Load Mahimahi config from TOML file if provided.
        if mm_config is not None:
            mm_cfg = tomli.load(open(mm_config, "rb"))
            self.disable_tc_client = mm_cfg.get("disable_tc_client", False)

            if mm_cfg["mm_bin"] == "cell":
                num_args = mm_cfg["mm_cell"]["NUM_ARGS"]
                up_packet_train_trace = mm_cfg["mm_cell"]["UP-PACKET-TRAIN-TRACE"]
                down_packet_train_trace = mm_cfg["mm_cell"]["DOWN-PACKET-TRAIN-TRACE"]
                up_pdo = mm_cfg["mm_cell"]["UP-PDO"]
                down_pdo = mm_cfg["mm_cell"]["DOWN-PDO"]
                psize_latency_offset_up = mm_cfg["mm_cell"]["psize_latency_offset_up"]
                psize_latency_offset_down = mm_cfg["mm_cell"][
                    "psize_latency_offset_down"
                ]

                self.client_mm_prefix = f"sudo -E -u {USER_NAME} /opt/cellreplay/bin/mm-cellular {num_args} {up_packet_train_trace} {down_packet_train_trace} {up_pdo} {down_pdo} --psize-latency-offset-up={psize_latency_offset_up} --psize-latency-offset-down={psize_latency_offset_down} bash -c 'sleep {WAIT_MM_INIT}; "
                self.client_mm_suffix = "'"
            elif mm_cfg["mm_bin"] == "leo":
                mm_bw_trace = mm_cfg["mm_leo"]["mm_bw_trace"]
                mm_delay_trace = mm_cfg["mm_leo"]["mm_delay_trace"]
                mm_delay_step_ms = mm_cfg["mm_leo"]["mm_delay_step_ms"]

                self.client_mm_prefix = f"mm-delay {mm_delay_step_ms} {mm_delay_trace} mm-link {mm_bw_trace} {mm_bw_trace} --uplink-queue droptail --uplink-queue-args packets={bdp_mm_pkts} --downlink-queue droptail --downlink-queue-args packets={bdp_mm_pkts} -- bash -c 'sleep {WAIT_MM_INIT}; "
                self.client_mm_suffix = "'"
            else:
                raise ValueError(f"Unsupported mm_bin: {mm_cfg['mm_bin']}")
        else:
            # Mahimahi disabled
            self.client_mm_prefix = ""
            self.client_mm_suffix = ""
            self.disable_tc_client = False

        assert (
            self.disable_tc_client is False or self.client_mm_prefix != ""
        ), "Cannot disable tc on client when mm is not enabled."

        # Optional seed for netem random loss. If set, appended to netem loss commands
        # as: "loss <x>% seed <loss_seed> ...". Default is None (no seed).
        self.loss_seed = loss_seed

        # Configure interfaces
        popen(self.r1, "ifconfig r1-eth0 0")
        popen(self.r1, "ifconfig r1-eth1 0")
        popen(self.r1, "ifconfig r1-eth0 hw ether 00:00:00:00:01:01")
        popen(self.r1, "ifconfig r1-eth1 hw ether 00:00:00:00:01:02")
        popen(self.r1, "ip addr add 10.0.1.1/24 brd + dev r1-eth0")
        popen(self.r1, "ip addr add 10.0.2.1/24 brd + dev r1-eth1")
        self.r1.cmd("echo 1 > /proc/sys/net/ipv4/ip_forward")
        popen(self.h1, "ip route add default via 10.0.1.1")  # client
        popen(self.h2, "ip route add default via 10.0.2.1")  # server

        # Need to set MTU when using Mahimahi (or the tests will fail)
        if self.client_mm_prefix != "":
            self.h2.cmdPrint("ip link set dev h2-eth0 mtu 1380")
            self.h1.cmdPrint("ip link set dev h1-eth0 mtu 1380")
            self.r1.cmdPrint("ip link set dev r1-eth0 mtu 1380")
            self.r1.cmdPrint("ip link set dev r1-eth1 mtu 1380")

        # If mahimahi enabled, set rule for ICMP packets from r1 to h1
        if self.client_mm_prefix != "":
            # r1 Echo Request go h1-ip: go 100.64.0.2
            self.h1.cmdPrint(
                f"iptables -t nat -A PREROUTING -p icmp --icmp-type echo-request -s 10.0.1.1 -d {self.h1.IP()} -j DNAT --to-destination 100.64.0.2"
            )

            # accept "-> 100.64.0.2" icmp packets
            self.h1.cmdPrint(
                f"iptables -A FORWARD -p icmp -d 100.64.0.2 -m conntrack --ctstate NEW,ESTABLISHED,RELATED -j ACCEPT"
            )

            # accept "100.64.0.2 ->" icmp packets
            self.h1.cmdPrint(
                f"iptables -A FORWARD -p icmp -s 100.64.0.2 -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT",
            )

        # Configure link latency, delay, bandwidth, and queue size

        def tc(host, iface, loss_set, delay, bw):
            netem_seed = f" seed {self.loss_seed}" if self.loss_seed is not None else ""
            if qdisc == "tbf":
                popen(
                    host,
                    f"tc qdisc add dev {iface} root handle 1:0 "
                    f"netem loss {loss_set}{netem_seed} delay {delay}ms",
                )
                popen(
                    host,
                    f"tc qdisc add dev {iface} parent 1:1 handle 10: "
                    f"tbf rate {bw}mbit burst {bw*500*2} limit {bdp}",
                )
            elif qdisc == "cake":
                popen(
                    host,
                    f"tc qdisc add dev {iface} root handle 1:0 "
                    f"netem loss {loss_set}{netem_seed} delay {delay}ms",
                )
                popen(
                    host,
                    f"tc qdisc add dev {iface} parent 1:1 handle 10: "
                    f"cake bandwidth {bw}mbit"
                    f"oceanic flowblind besteffort",
                )
            elif qdisc == "codel":
                popen(
                    host,
                    f"tc qdisc add dev {iface} root handle 1:0 "
                    f"netem loss {loss_set}{netem_seed} delay {delay}ms rate {bw}mbit",
                )
                popen(host, f"tc qdisc add dev {iface} parent 1:1 handle 10: codel")
            elif qdisc == "red":
                popen(
                    host,
                    f"tc qdisc add dev {iface} handle 1:0 root "
                    f"red limit {bdp*4} avpkt 1000 adaptive "
                    f"harddrop bandwidth {bw}Mbit",
                )
                popen(
                    host,
                    f"tc qdisc add dev {iface} parent 1:1 handle 10: "
                    f"netem loss {loss_set}{netem_seed} delay {delay}ms rate {bw}mbit",
                )
            elif qdisc == "grenville":
                popen(
                    host,
                    f"tc qdisc add dev {iface} root handle 2: netem loss {loss_set}{netem_seed} delay {delay}ms",
                )
                popen(
                    host, f"tc qdisc add dev {iface} parent 2: handle 3: htb default 10"
                )
                popen(
                    host,
                    f"tc class add dev {iface} parent 3: classid 10 htb rate {bw}Mbit",
                )
                popen(
                    host,
                    f"tc qdisc add dev {iface} parent 3:10 handle 11: "
                    f"red limit {bdp*4} avpkt 1000 adaptive harddrop bandwidth {bw}Mbit",
                )
            else:
                pemilog("{} {} no qdisc enabled".format(host, iface))

        if not self.disable_tc_client:
            if loss1ge is None:
                loss1_set = f"{loss1}%"
            else:
                # 4 paras: good->bad, bad->good, bad_loss, good_loss
                loss1_set = (
                    f"gemodel {loss1ge[0]}% {loss1ge[1]}% {loss1ge[2]}% {loss1ge[3]}%"
                )
            tc(self.h1, "h1-eth0", loss1_set, delay1, bw1)
            tc(self.r1, "r1-eth0", loss1_set, delay1, bw1)
        tc(self.r1, "r1-eth1", f"{loss2}%", delay2, bw2)
        tc(self.h2, "h2-eth0", f"{loss2}%", delay2, bw2)

    def del_tc(self):
        if not self.disable_tc_client:
            popen(self.h1, "tc qdisc del dev h1-eth0 root")
            popen(self.r1, "tc qdisc del dev r1-eth0 root")
        popen(self.r1, "tc qdisc del dev r1-eth1 root")
        popen(self.h2, "tc qdisc del dev h2-eth0 root")

    def stop(self):
        if self.net is not None:
            self.net.stop()
        if self.client_mm_prefix != "":
            os.system(
                "sudo sysctl -w net.ipv4.ip_forward=0"
            )  # disable forward after use mahimahi

    def init_arp(self):
        # init ARPs
        self.h1.cmdPrint(f"ping -c1 {self.h2.IP()}")

    def run_ping(self, num_pings=5):
        """
        Run a ping reachability test between all hosts.
        """
        self.h2.cmdPrint(
            f"{self.client_mm_prefix} ping -c{num_pings} {self.h1.IP()}{self.client_mm_suffix}"
        )
        self.h1.cmdPrint(
            f"{self.client_mm_prefix} ping -c{num_pings} {self.h2.IP()}{self.client_mm_suffix}"
        )

    def run_iperf(self, time_s, host, max=False):
        self.h2.cmd("iperf3 -s -f m > /dev/null 2>&1 &")
        if host == "r1":
            host = self.r1
        elif host == "h1":
            host = self.h1
        else:
            exit(1)
        if max:
            bitrate = ""  # no limit
            # del tc settings
            self.del_tc()
        else:
            bitrate = "-b 100M"
        host.cmdPrint(
            f"{self.client_mm_prefix} iperf3 -c {self.h2.IP()} -t {time_s} -f m {bitrate} -C cubic -i 1 {self.client_mm_suffix}"
        )

    def reroute(self, protocols):
        popen(self.r1, "ip rule add fwmark 1 lookup 100")
        popen(self.r1, "ip route add local 0.0.0.0/0 dev lo table 100")
        popen(self.r1, "iptables -t mangle -F")
        for protocol in protocols:
            popen(
                self.r1,
                f"iptables -t mangle -A PREROUTING -i r1-eth1 -p {protocol} -j TPROXY --on-port 5000 --tproxy-mark 1",
            )
            popen(
                self.r1,
                f"iptables -t mangle -A PREROUTING -i r1-eth0 -p {protocol} -j TPROXY --on-port 5000 --tproxy-mark 1",
            )

    def start_tcp_pep(self):
        pemilog("Starting the TCP PEP on r1...")
        self.reroute(["tcp"])
        self.r1.cmd("pepsal -v >> r1.log 2>&1 &")

    def start_pemi(
        self, log_level, fl_inv_factor, fl_end_factor, pemi_proxy_only=False
    ):
        pemilog("Starting the PEMI on r1...")
        self.reroute(["udp", "tcp"])  # set tcp because of the need of test
        # make sure the icmp can be used by the pemi
        self.r1.cmdPrint(f'sudo sysctl -w net.ipv4.ping_group_range="0 2147483647"')
        self.r1.cmd(f"kill $(pidof pemi)")
        # clear the log file
        self.h2.cmd("rm -f r1.log")
        # log_level = "error"
        proxy_only = ""  # default not set, and the proxy-only will be closed
        if pemi_proxy_only:
            proxy_only = "--proxy-only"  # set to true
        cmd = f"RUST_BACKTRACE=1 RUST_LOG={log_level} taskset -c {multiprocessing.cpu_count() - 1} ./target/release/pemi --fl-inv-factor {fl_inv_factor} --fl-end-factor {fl_end_factor}  {proxy_only}  &> r1.log &"
        # cmd = f"./pemi/temp/pemi &> r1.log &"
        pemilog(cmd)
        self.r1.cmd(cmd)
        # wait start
        while True:
            try:
                with open("r1.log", "r") as f:
                    if "listening on" in f.read():
                        return
            except FileNotFoundError:
                pass  # file not created yet
            time.sleep(0.1)

    def start_webserver(self):
        pemilog("Starting the NGINX/Python webserver on h2...")
        self.h2.cmdPrint("kill $(pidof nginx)")
        # make the tcp and udp(for quiche) mtu the same
        self.h2.cmdPrint("ip link set dev h2-eth0 mtu 1250")  # set MTU to 1250
        self.h1.cmdPrint("ip link set dev h1-eth0 mtu 1250")  # set MTU to 1250

        # get this directory
        current_dir = os.getcwd()
        nginx_conf = current_dir + "/apps/http/nginx.conf"
        self.h2.cmdPrint(f"nginx -c {nginx_conf}")
        self.h2.cmdPrint(f"python3 apps/http/http_server.py &> s1.log &")
        while True:
            try:
                with open("s1.log", "r") as f:
                    if "Starting httpd" in f.read():
                        return
            except FileNotFoundError:
                pass  # file not created yet
            time.sleep(0.1)

    def start_quiche_rtc_server(self, log_level, start_time):
        """
        Start the server on h2. Make sure the server is ready before returning.
        """
        pemilog("Starting the server on h2...")
        # clear the log file
        self.h2.cmd("rm -f s1.log")
        self.h2.cmdPrint(
            f"RUST_LOG={log_level} ./target/release/rtc_server -s {start_time} -p {self.h2.IP()}:4433 &> s1.log &"
        )
        while True:
            try:
                with open("s1.log", "r") as f:
                    if "Listening on" in f.read():
                        return
            except FileNotFoundError:
                pass  # file not created yet
            time.sleep(0.1)

    def start_quiche_rtc_client(self, log_level, start_time, video_long=10):
        frames = 30 * video_long
        pemilog("Starting the client on h1...")
        self.h1.cmdPrint(
            f"RUST_LOG={log_level} {self.client_mm_prefix}./target/release/rtc_client {start_time} http://{self.h2.IP()}:4433 {frames} &> c1.log{self.client_mm_suffix}"
        )

    def start_capture(self, args):
        """
        Start capturing packets on all nodes.
        """
        # clear old files (logs and pcap files)
        pcap_dir = "pcap"
        self.h1.cmd(f"rm -f {pcap_dir}/h1-eth0.pcap cap_h1.log")
        self.h2.cmd(f"rm -f {pcap_dir}/h2-eth0.pcap cap_h2.log")
        self.r1.cmd(f"rm -f {pcap_dir}/r1-eth0.pcap cap_r1.log")
        self.r1.cmd(f"rm -f {pcap_dir}/r1-eth1.pcap cap_r1-eth1.log")
        # start capturing
        cap_filter = f"ip host {self.h1.IP()} and ip host {self.h2.IP()}"
        # if args.pemi or args.pep:
        #     cap_filter = f""  # remove all rules if the addr change
        self.h1.cmd(
            f"tshark -i h1-eth0 -f '{cap_filter}' -w {pcap_dir}/h1-eth0.pcap &> cap_h1.log &"
        )
        self.h2.cmd(
            f"tshark -i h2-eth0 -f '{cap_filter}' -w {pcap_dir}/h2-eth0.pcap &> cap_h2.log &"
        )
        self.r1.cmd(
            f"tshark -i r1-eth0 -f '{cap_filter}' -w {pcap_dir}/r1-eth0.pcap &> cap_r1.log &"
        )
        self.r1.cmd(
            f"tshark -i r1-eth1 -f '{cap_filter}' -w {pcap_dir}/r1-eth1.pcap &> cap_r1-eth1.log &"
        )
        # wait the capture to start
        check_cap_start("cap_r1.log", "tshark")
        check_cap_start("cap_h1.log", "tshark")
        check_cap_start("cap_h2.log", "tshark")
        check_cap_start("cap_r1-eth1.log", "tshark")
