# Mininet Environment for PEMI Experiments

## CellReplay / LeoReplayer Integration

CellReplay and LeoReplayer are both network emulation tools built on Mahimahi. When integrating mahimahi-like emulators, we launch the client application through the shell provided by these tools. Please ensure that the corresponding emulators are correctly installed and configured on your system.

In our setup, CellReplay is installed at `/opt/cellreplay/bin/mm-cellular`, and LeoReplayer is installed in the system path. Modify the paths to the corresponding binaries in `mininet/network.py` according to your own installation.  

In addition, CellReplay must be executed under a normal (non-root) user account, so be sure to specify a correct username in `mininet/network.py` to run CellReplay. Also, due to permission issues when using different users, you may need to delete temporary files created by CellReplay before and after CellReplay tests, e.g., via `make clear`.

For CellReplay installation, refer to: https://github.com/williamsentosa95/cellreplay  
For LeoReplayer installation, refer to: https://github.com/SpaceNetLab/LeoCC

## Configuring the Trace-driven Emulators

Because the parameters for CellReplay and LeoReplayer are relatively complex, we use `.toml` files to configure them—primarily to specify the datasets being used.  
The required parameters and available datasets for each emulator can be found at:

1. https://github.com/williamsentosa95/cellreplay  
2. https://github.com/SpaceNetLab/LeoCC

Please download and place the trace files in the same directories as the ones provided in `.toml` file. You can then enable the trace-drive experiment by specifying the corresponding `.toml` file, e.g., `--mm-config mininet/mahimahi/cell_tmobile_driving.toml`.
