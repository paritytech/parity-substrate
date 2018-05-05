# Polkadot

Implementation of a https://polkadot.io node in Rust.

## To play

If you'd like to play with Polkadot, you'll need to install a client like this
one. First, get rust if you don't already have it:

   curl https://sh.rustup.rs -sSf | sh

Then, install Polkadot PoC-1:

   cargo install --git https://github.com/paritytech/polkadot.git --branch v0.1.0

You'll now have a `polkadot` binary installed to your `PATH`. You can drop the
`--branch v0.1.0` to get the very latest version of Polkadot, but these
instructions might not work in that case.

### Development

You can run a simple single-node development "network" on your machine by running in
one terminal:

   polkadot --chain=dev --validator --key Alice

You can muck around by cloning and building the http://github.com/paritytech/polka-ui and http://github.com/paritytech/polkadot-ui or just heading to https://polkadot.js.org/apps.

### PoC-1 Testnet

You can also connect to the global PoC-1 testnet. To do this, just use:

   polkadot --chain=poc-1

If you want to do anything on it (not that there's much to do), then you'll need
to get some PoC-1 testnet DOTs. Ask in the Polkadot watercooler.

## Local Two-node Testnet

If you want to see the multi-node consensus algorithm in action locally, then
you can create a local testnet. You'll need two terminals open. In one, run:

   polkadot --chain=dev --validator --key Alice -d /tmp/alice

and in the other, run:

   polkadot --chain=dev --validator --key Bob -d /tmp/bob --port 30334 --bootnodes 'enode://ALICE_BOOTNODE_ID_HERE@127.0.0.1:30333'

Ensure you replace `ALICE_BOOTNODE_ID_HERE` with the node ID from the output of
the first terminal.
