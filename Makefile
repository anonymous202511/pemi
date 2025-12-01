build:
	cargo build
	cd apps/quinn-apps && cargo build
	cd apps/quicgo-apps && make

release:
	cargo build --release
	cd apps/quinn-apps && cargo build --release
	cd apps/quicgo-apps && make

clean:
	cargo clean

clear:
	sudo rm -f *.log pcap/*.pcap *.csv
	sudo rm -f /tmp/pemi* # http server and client