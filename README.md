# Neptune Legacy

[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![GitHub CI](https://github.com/Neptune-Crypto/neptune-core/actions/workflows/main.yml/badge.svg)](https://github.com/Neptune-Crypto/neptune-core/actions/workflows/main.yml)
[![crates.io](https://img.shields.io/crates/v/neptune-cash.svg)](https://crates.io/crates/neptune-cash)
[![Coverage Status](https://coveralls.io/repos/github/Neptune-Crypto/neptune-core/badge.svg?branch=master)](https://coveralls.io/github/Neptune-Crypto/neptune-core?branch=master)

Neptune-legacy is the reference implementation for the legacy version of the
[Neptune Cash](https://neptune.cash) protocol, which was live from 2025-02-11 until 2025-07-04 when
an [undetectable inflation bug](https://neptune.cash/blog/inflation-bug-discovered/) was discovered.
The network lives on for owners of UTXOs to exercise
[UTXO redemption claims](https://neptune.cash/blog/utxo-redemption/) and be credited with on the
[rebooted network](https://github.com/Neptune-Crypto/neptune-core).

## Disclaimer

> [!CAUTION]
> This software uses novel and untested cryptography. Use at own risk, and invest only that which
> you can afford to lose.

> [!IMPORTANT]
> If a catastrophic vulnerability is discovered in the protocol, it will restart from genesis.

## Installing

Installation from released binaries is easiest and recommended. If that does not work, or if you need features that live
on master but have not been released yet, you will need to compile from source.

### Released Binaries

 - Go to the [releases page](https://github.com/Neptune-Crypto/neptune-legacy/releases).
 - Run the suggested (power)shell script; or download and install the installer matching with your platform.

### Compile from Source -- Linux Debian/Ubuntu

- Open a terminal to run the following commands.
- Install curl: `sudo apt install curl`
- Install the rust compiler and accessories:
  `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y`
- Source the rust environment: `source "$HOME/.cargo/env"`
- Install build tools: `sudo apt install build-essential`
- Install LevelDB: `sudo apt install libleveldb-dev libsnappy-dev cmake`
- Download the repository: `git clone https://github.com/Neptune-Crypto/neptune-core.git`
- Enter the repository: `cd neptune-core`
- Checkout the release branch `git checkout release`. (Alternatively, for the *unstable development*
  branch, skip this step.)
- Build for release and put the binaries in your local path (`~/.cargo/bin/`):
  `cargo install --locked --path .` (needs at least 3 GB of RAM and a few minutes)

> [!IMPORTANT]
> Any branch except tag `release` is considered an _unstable development_ branch. Should you choose to use
> such a branch, you risk database corruption, loss of funds, crashing user interfaces, _etc_.

### Compile from Source -- Windows

To install Rust and cargo on Windows, you can
follow [these instructions](https://doc.rust-lang.org/cargo/getting-started/installation.html).
Installing cargo might require you to install Visual Studio with some C++ support but the cargo
installer for Windows should handle that. With a functioning version of cargo, compilation on
Windows should just work out-of-the-box with cargo build etc.

- Download and run the CMake installer from the [website](https://cmake.org/download/).
- Open PowerShell to run the following commands.
- Download the repository: `git clone https://github.com/Neptune-Crypto/neptune-core.git`
- Enter the repository: `cd neptune-core`
- Checkout the release tag `git checkout release`. (Alternatively, for an *unstable development*
  branch, skip this step.)

- Run `cargo install --locked --path .`

## Running & Connecting

- Run neptune-core daemon: `neptune-core` with flags
    - `--peers [ip_address:port]` to connect to a given peer, for instance
    `--peers 139.162.193.206:19798`
    - `--help` to get a list of available command-line arguments

If you don't have a static IPv4, then try connecting to other nodes with IPv6. It's our experience
that you will then be able to open and receive connections to other nodes through Nepture Core's
built-in peer-discovery process.

## Dashboard

This software comes with a dashboard that communicates with the daemon. The dashboard is a
console-based user interface to generate addresses, receive and send money, and monitor the behavior
of the client. The daemon must be running before the dashboard is started. To start the dashboard,
run: `neptune-dashboard`. (If you set daemon's RPC port to a custom value specify that value with
the flag `--port [port]`.)

## Command-Line Interface

In addition to a dashboard, the software comes with a CLI client to invoke procedures in the daemon.
This can be invoked from another terminal window when the daemon is running. To get all available
commands, execute

```
neptune-cli --help
```

To get e.g. the block height of a running daemon, execute

```
neptune-cli block-height
```

If you set up `neptune-core` to listen for RPC requests on a different port from the default (9799),
then the flag `--port <port>` is your friend.

## Restarting Node from the Genesis Block

If you are not already synced to the network, you want to restart your node from genesis. To do
that, start by delete these folders.

- `<data_directory>/main/blocks/`
- `<data_directory>/main/databases/`

Note that all data required to retrieve your funds is located in the following files:

- `<data_directory>/main/wallet/wallet.dat`
- `<data_directory>/main/wallet/incoming_randomness.dat`
- `<data_directory>/main/wallet/outgoing_randomness.dat`.

Therefore, the `blocks/` and `databases/` directories can safely be deleted without risking loss of
funds -- as long as the files in `wallet/` remain.

On Linux, with the standard settings, the `data_directory` is `~/.local/share/neptune/`. On every
system the data directory is outputted by the node at startup, so you can read it from the log.

## Syncing

If you start the node with the command-line argument `--peers 139.162.193.206:19798`, then you will
connect to at least one legacy peer. If there are other nodes on the network, your node will
discover them automatically through peer discovery.

Meanwhile, your node will automatically enter into sync mode if it notices a big discrepancy between
the block height of its peers and its own block height. When in sync mode, the node will start
downloading historical blocks until it has caught up with the tip. On legacy, catching up to block
21310 suffices; anything after that is wasted effort.

While syncing should start and continue automatically, the process is not as robust as it could be.
If you run into issues while syncing,
[sync timeouts](https://github.com/Neptune-Crypto/neptune-core/issues/634) or even crashes, it is
fine to restart the node periodically.

As an alternative to the regular sync mode initial block download, you can also download the blocks
via torrent and sync from there.

## Torrent and Sync

 - Install your favorite Bittorrent client. (Vuze, Transmission, Bittorrent, etc.).
 - Download the
   [torrent file](https://neptune.cash/blog/utxo-redemption/neptune-cash-legacy-blocks20250728.torrent).
 - Use the Bittorrent client to download the torrent.
 - Unpack the archive. On linux: `tar -xf neptune-mainnet-blocks-sword-smith-2025-07-28.tar.gz`
 - This will generate a directory called `home/` which looks empty, but don't be fooled. The blocks
   are located at `home/thv/.local/share/neptune/main/blocks/`.
 - Start your Neptune Legacy node with the command-line argument
   `--bootstrap-from-directory path/to/thv/.local/share/neptune/main/blocks/`.

## Exercising a UTXO Redemption Claim

Note that earlier instructions are [here](https://neptune.cash/blog/utxo-redemption/) and
[here](https://neptune.cash/learn/utxo-redemption-tutorial/). You might want to cross-reference
against these resources.

### Pre-requisites

 - A running legacy node synced to block 21310 (or later).
 - >5 GB of free RAM.
 - A terminal for using the command-line interface.
 - The command-line interface, `neptune-cli`.

### Instructions

 1. Set tip to block 21310: `neptune-cli set-tip ecfae777da1a6b5ad97d3d793cb64b0cb4262ac5e378d2f4e2a5049e731298e058b2000000000000`.

This sets the blockchain-state of the node to block 21310. It will also freeze the node, so that 
announced blocks and transactions are ignored.

 2. Produce the UTXO redemption claim: `neptune-cli redeem-utxos`.

Note that the CLI command will return quickly, but the the running node will start producing
redemption claims. Specifically, redemption claims live in files called somthing like
`b64b0cb4262ac5e378d.redeem`. They take a while to produce (because they consist of a lot of
zk-STARK proofs), so wait a while before the redeem file appears.

Generally speaking you get one redemption claim per wallet. So if re-execute the command, you will
generate a new redeem file with a new name, but this redeem file is essentially the same claim as
the first one. You can only submit one redemption per UTXO.

Every redemption claim is tailored to a receiving address, which is the one that will be credited on
the reboot network. By default the receiving address is identical to the running node's 0th
generation address, which you can display with `neptune-cli premine-receiving-address`. If you want
to supply an alternate address, then call `redeem-utxos` with flag `--address <alternate-address>`.

The redeem file is stored to a directory which is by default `redemption-claims/`. You can override
this directory with the flag `--directory <other-directory/>`.

 3. Verify the UTXO redemption claim: `neptune-cli verify-redemption`.

This command will instruct the running node to read all redeem files from the directory
(`redemption-claims/` or whatever it was overridden to) and verify them. If all claims are valid and
mutually compatible, it will produce a report of all claims and amounts. If you're just interested
in producing and submitting your own claim, this report will contain one line.

 4. Send the redeem file to us.
