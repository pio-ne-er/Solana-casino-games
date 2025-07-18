import { program } from "commander";
import { initProject, setClusterConfig, update, withdraw } from "./scripts";

program.version("0.0.1");

programCommand("initialize")
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  .action(async (directory, cmd) => {
    const { env, keypair, rpc } = cmd.opts();
    console.log("Solana Env Config:", env);
    console.log("Keypair Path:", keypair);
    console.log("RPC URL:", rpc);
    if (keypair === undefined || rpc === undefined) {
      console.log("Error Config Data Input");
      return;
    }
    await setClusterConfig(env, keypair, rpc);

    await initProject();
  });

programCommand("update")
  .option("-a, --address <string>", "admin address")
  .option("-f, --fee <number>", "set the loyalty fee[2.5 means 2.5%]")

  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  .action(async (directory, cmd) => {
    const { env, keypair, rpc, address, fee } = cmd.opts();
    console.log("Solana Env Config:", env);
    console.log("Keypair Path:", keypair);
    console.log("RPC URL:", rpc);
    if (keypair === undefined || rpc === undefined) {
      console.log("Error Config Data Input");
      return;
    }

    await setClusterConfig(env, keypair, rpc);
    if (address === undefined) {
      console.log("Error Admin Address Input");
      return;
    }

    if (fee === undefined || isNaN(parseFloat(fee))) {
      console.log("Error Fee Input");
      return;
    }

    await update(address, parseFloat(fee));
  });

programCommand("withdraw")
  .option("-a, --amount <number>", "amount to withdraw")
  // eslint-disable-next-line @typescript-eslint/no-unused-vars
  .action(async (directory, cmd) => {
    const { env, keypair, rpc, amount } = cmd.opts();
    console.log("Solana Env Config:", env);
    console.log("Keypair Path:", keypair);
    console.log("RPC URL:", rpc);
    if (keypair === undefined || rpc === undefined) {
      console.log("Error Config Data Input");
      return;
    }

    await setClusterConfig(env, keypair, rpc);
    if (amount === undefined || isNaN(parseInt(amount))) {
      console.log("Error Purpose Input");
      return;
    }

    await withdraw(parseFloat(amount));
  });

function programCommand(name: string) {
  return program
    .command(name)
    .option("-e, --env <string>", "Solana cluster env name", "mainnet-beta") //mainnet-beta, testnet, devnet
    .option(
      "-r, --rpc <string>",
      "Solana cluster RPC name",
      "https://api.mainnet-beta.solana.com"
    ) //https://api.devnet.solana.com |
    .option(
      "-k, --keypair <string>",
      "Solana wallet Keypair Path",
      "/Users/kob/.config/solana/SERVUJeqsyaJTuVuXAmmko6kTigJmxzTxUMSThpC2LZ.json"
    );
}

program.parse(process.argv);

