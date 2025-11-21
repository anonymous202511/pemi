# Mininet Environment for PEMI Experiments

## CellReplay / LeoReplayer Integration

CellReplay and LeoReplayer are both network emulation tools built on Mahimahi. When integrating mahimahi-like emulators, we launch the client application through the shell provided by these tools. Please ensure that the corresponding emulators are correctly installed and configured on your system.

In our setup, CellReplay is installed at `/opt/cellreplay/bin/mm-cellular`, and LeoReplayer is installed in the system path. Modify the paths to the corresponding binaries in `mininet/network.py` according to your own installation.  
In addition, CellReplay must be executed under a normal (non-root) user account, so be sure to specify a correct username in `mininet/network.py` to run CellReplay.

For CellReplay installation, refer to: https://github.com/williamsentosa95/cellreplay  
For LeoReplayer installation, refer to: https://github.com/SpaceNetLab/LeoCC
