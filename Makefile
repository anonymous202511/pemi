build:
	cargo build

release:
	cargo build --release

clean:
	cargo clean

clear:
	sudo rm -f *.log pcap/*.pcap *.csv
	sudo rm -f /tmp/pemi* # http server and client