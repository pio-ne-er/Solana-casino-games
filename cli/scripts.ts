import { Program, translateAddress, web3 } from "@coral-xyz/anchor";
import * as anchor from "@coral-xyz/anchor";
import {
  Keypair,
  PublicKey,
  SystemProgram,
  SYSVAR_RENT_PUBKEY,
  PartiallyDecodedInstruction,
  Transaction,
} from "@solana/web3.js";
import fs from "fs";
import NodeWallet from "@coral-xyz/anchor/dist/cjs/nodewallet";
import { GlobalPool, PlayerPool } from "./types";
import { IDL as GameIDL } from "../target/types/coinflip";
const IDLJson = require("../target/idl/coinflip.json");
import { bs58 } from "@coral-xyz/anchor/dist/cjs/utils/bytes";

const PLAYER_POOL_SIZE = 112;
const LAMPORTS = 1000000000;
const GLOBAL_AUTHORITY_SEED = "global-authority";
const VAULT_AUTHORITY_SEED = "vault-authority";
const PLAYER_POOL_SEED = "player-pool";
const NONCE = "4QUPibxi";

const network = "devnet";
const PROGRAM_ID = IDLJson.metadata.address;

// Set the initial program and provider
let program: Program = null;
let provider: anchor.Provider = null;

// Address of the deployed program.
let programId = new anchor.web3.PublicKey(PROGRAM_ID);

anchor.setProvider(anchor.AnchorProvider.local(web3.clusterApiUrl(network)));
provider = anchor.getProvider();

let solConnection = anchor.getProvider().connection;

// Generate the program client from IDL.
program = new anchor.Program(GameIDL as anchor.Idl, programId);
console.log("ProgramId: ", program.programId.toBase58());

const main = async () => {
  const [globalAuthority, bump] = await PublicKey.findProgramAddress(
    [Buffer.from(GLOBAL_AUTHORITY_SEED)],
    program.programId
  );
  console.log("GlobalAuthority: ", globalAuthority.toBase58());

  const [rewardVault, vaultBump] = await PublicKey.findProgramAddress(
    [Buffer.from(VAULT_AUTHORITY_SEED)],
    program.programId
  );
  console.log("RewardVault: ", rewardVault.toBase58());

  // await initProject();

  // const globalPool: GlobalPool = await getGlobalState();
  // console.log("GlobalPool Admin =", globalPool.superAdmin.toBase58(), globalPool.totalRound.toNumber());

  // await initUserPool(payer.publicKey);
  // await playGame(provider.publicKey, 1, 0.1);
  // await claim(provider.publicKey);
  // await withDraw(payer.publicKey, 0.5);
  // console.log(await getAllTransactions(program.programId));
  // console.log(await getDataFromSignature('2d88xKvDRPAc5rM2XzGqjNTinZXVSkKUrwYTpFAzT3SwuJLbob2xocfbgBwWfdJwFccew2SispiRC2E3H1uD6Drt'));
};

export const setClusterConfig = async (
  cluster: web3.Cluster,
  keypair: string,
  rpc?: string
) => {
  if (!rpc) {
    solConnection = new web3.Connection(web3.clusterApiUrl(cluster));
  } else {
    solConnection = new web3.Connection(rpc);
  }

  //console.log("Connection", solConnection);
  const walletKeypair = Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(keypair, "utf-8"))),
    { skipValidation: true }
  );
  const wallet = new NodeWallet(walletKeypair);

  // Configure the client to use the local cluster.
  anchor.setProvider(
    new anchor.AnchorProvider(solConnection, wallet, {
      skipPreflight: true,
      commitment: "confirmed",
    })
  );
  provider = anchor.getProvider();
  console.log("Wallet Address: ", wallet.publicKey.toBase58());

  // Generate the program client from IDL.
  program = new anchor.Program(GameIDL as anchor.Idl, programId);
  console.log("ProgramId: ", program.programId.toBase58());
};

export const initProject = async () => {
  const [globalAuthority, bump] = await PublicKey.findProgramAddress(
    [Buffer.from(GLOBAL_AUTHORITY_SEED)],
    program.programId
  );
  const [rewardVault, vaultBump] = await PublicKey.findProgramAddress(
    [Buffer.from(VAULT_AUTHORITY_SEED)],
    program.programId
  );

  let tx = new Transaction();

  tx.add(
    program.instruction.initialize({
      accounts: {
        admin: provider.publicKey,
        globalAuthority,
        rewardVault: rewardVault,
        systemProgram: SystemProgram.programId,
        rent: SYSVAR_RENT_PUBKEY,
      },
      signers: [],
    })
  );

  const txId = await provider.sendAndConfirm(tx, [], {
    commitment: "confirmed",
  });

  console.log("txHash =", txId);

  return true;
};

export const initializeUserPool = async (userAddress: PublicKey) => {
  const tx = await initUserPoolTx(userAddress);
  const txId = await provider.sendAndConfirm(tx, [], {
    commitment: "confirmed",
  });

  console.log("txHash =", txId);
};

export const update = async (loyaltyWallet: PublicKey, loyaltyFee: number) => {
  const tx = await updateTx(provider.publicKey, loyaltyWallet, loyaltyFee);
  const txId = await provider.sendAndConfirm(tx, [], {
    commitment: "confirmed",
  });

  console.log("txHash =", txId);
};

export const playGame = async (
  userAddress: PublicKey,
  setValue: number,
  deposit: number
) => {
  const tx = await createPlayGameTx(userAddress, setValue, deposit);
  const txId = await provider.sendAndConfirm(tx, [], {
    commitment: "confirmed",
  });

  console.log("txHash =", txId);
  // let userPoolData = await program.account.playerPool.fetch(playerPoolKey);
  // console.log(userPoolData.gameData);
};

export const claim = async (userAddress: PublicKey) => {
  const tx = await createClaimTx(userAddress);
  const txId = await provider.sendAndConfirm(tx, [], {
    commitment: "confirmed",
  });

  console.log("txHash =", txId);
};

export const withdraw = async (amount: number) => {
  const tx = await createWithDrawTx(provider.publicKey, amount);
  const txId = await provider.sendAndConfirm(tx, [], {
    commitment: "confirmed",
  });

  console.log("txHash =", txId);
};

//////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////
//////////////////////////////////////////////////////////

export const initUserPoolTx = async (userAddress: PublicKey) => {
  let playerPoolKey = await PublicKey.createWithSeed(
    userAddress,
    PLAYER_POOL_SEED,
    program.programId
  );
  console.log(playerPoolKey.toBase58());

  let tx = new Transaction();

  let ix = SystemProgram.createAccountWithSeed({
    fromPubkey: userAddress,
    basePubkey: userAddress,
    seed: PLAYER_POOL_SEED,
    newAccountPubkey: playerPoolKey,
    lamports: await solConnection.getMinimumBalanceForRentExemption(
      PLAYER_POOL_SIZE
    ),
    space: PLAYER_POOL_SIZE,
    programId: program.programId,
  });

  tx.add(ix);
  tx.add(
    program.instruction.initializePlayerPool({
      accounts: {
        owner: userAddress,
        playerPool: playerPoolKey,
      },
      instructions: [],
      signers: [],
    })
  );

  return tx;
};

export const updateTx = async (
  userAddress: PublicKey,
  loyaltyWallet: PublicKey,
  loyaltyFee: number
) => {
  const [globalAuthority, bump] = await PublicKey.findProgramAddress(
    [Buffer.from(GLOBAL_AUTHORITY_SEED)],
    program.programId
  );

  let tx = new Transaction();

  tx.add(
    program.instruction.update(new anchor.BN(loyaltyFee * 10), {
      accounts: {
        admin: userAddress,
        globalAuthority,
        loyaltyWallet,
      },
      instructions: [],
      signers: [],
    })
  );

  return tx;
};

export const createPlayGameTx = async (
  userAddress: PublicKey,
  setNum: number,
  deposit: number
) => {
  const [globalAuthority, bump] = await PublicKey.findProgramAddress(
    [Buffer.from(GLOBAL_AUTHORITY_SEED)],
    program.programId
  );
  console.log("GlobalAuthority: ", globalAuthority.toBase58());

  const [rewardVault, vaultBump] = await PublicKey.findProgramAddress(
    [Buffer.from(VAULT_AUTHORITY_SEED)],
    program.programId
  );
  console.log("RewardVault: ", rewardVault.toBase58());

  let playerPoolKey = await PublicKey.createWithSeed(
    userAddress,
    PLAYER_POOL_SEED,
    program.programId
  );
  console.log(playerPoolKey.toBase58());

  const state = await getGlobalState();

  let tx = new Transaction();
  let poolAccount = await solConnection.getAccountInfo(playerPoolKey);
  if (poolAccount === null || poolAccount.data === null) {
    console.log("init User Pool");
    let tx1 = await initUserPoolTx(userAddress);
    tx.add(tx1);
  }

  tx.add(
    program.instruction.playGame(
      new anchor.BN(setNum),
      new anchor.BN(deposit * LAMPORTS),
      {
        accounts: {
          owner: userAddress,
          playerPool: playerPoolKey,
          globalAuthority,
          rewardVault: rewardVault,
          loyaltyWallet: state.loyaltyWallet,
          systemProgram: SystemProgram.programId,
        },
        signers: [],
      }
    )
  );

  return tx;
};

export const createClaimTx = async (userAddress: PublicKey) => {
  const [globalAuthority, bump] = await PublicKey.findProgramAddress(
    [Buffer.from(GLOBAL_AUTHORITY_SEED)],
    program.programId
  );
  console.log("GlobalAuthority: ", globalAuthority.toBase58());

  const [rewardVault, vaultBump] = await PublicKey.findProgramAddress(
    [Buffer.from(VAULT_AUTHORITY_SEED)],
    program.programId
  );

  let playerPoolKey = await PublicKey.createWithSeed(
    userAddress,
    PLAYER_POOL_SEED,
    program.programId
  );
  console.log(playerPoolKey.toBase58());
  let tx = new Transaction();

  console.log("===> Claiming The Reward");
  tx.add(
    program.instruction.claimReward({
      accounts: {
        owner: userAddress,
        playerPool: playerPoolKey,
        globalAuthority,
        rewardVault: rewardVault,
        systemProgram: SystemProgram.programId,
        instructionSysvarAccount: anchor.web3.SYSVAR_INSTRUCTIONS_PUBKEY,
      },
    })
  );

  return tx;
};

export const createWithDrawTx = async (
  userAddress: PublicKey,
  deposit: number
) => {
  const [globalAuthority, bump] = await PublicKey.findProgramAddress(
    [Buffer.from(GLOBAL_AUTHORITY_SEED)],
    program.programId
  );
  console.log("GlobalAuthority: ", globalAuthority.toBase58());

  const [rewardVault, vaultBump] = await PublicKey.findProgramAddress(
    [Buffer.from(VAULT_AUTHORITY_SEED)],
    program.programId
  );
  let tx = new Transaction();

  console.log("===> Withdrawing Sol");
  tx.add(
    program.instruction.withdraw(new anchor.BN(deposit * LAMPORTS), {
      accounts: {
        admin: userAddress,
        globalAuthority,
        rewardVault: rewardVault,
        systemProgram: SystemProgram.programId,
      },
    })
  );
  return tx;
};

export const getGlobalState = async (): Promise<GlobalPool | null> => {
  const [globalAuthority, bump] = await PublicKey.findProgramAddress(
    [Buffer.from(GLOBAL_AUTHORITY_SEED)],
    program.programId
  );
  try {
    let globalState = await program.account.globalPool.fetch(globalAuthority);
    return globalState as unknown as GlobalPool;
  } catch {
    return null;
  }
};

export const getUserPoolState = async (
  userAddress: PublicKey
): Promise<PlayerPool | null> => {
  if (!userAddress) return null;

  let playerPoolKey = await PublicKey.createWithSeed(
    userAddress,
    PLAYER_POOL_SEED,
    program.programId
  );
  console.log("Player Pool: ", playerPoolKey.toBase58());
  try {
    let poolState = await program.account.playerPool.fetch(playerPoolKey);
    return poolState as unknown as PlayerPool;
  } catch {
    return null;
  }
};

// Get signautres related with Program Pubkey
export const getAllTransactions = async (programId: PublicKey) => {
  const data = await solConnection.getSignaturesForAddress(
    programId,
    {},
    "confirmed"
  );
  let result = [];
  console.log(`Tracked ${data.length} signature\nStart parsing Txs....`);
  let txdata = data.filter((tx) => tx.err === null);
  for (let i = 0; i < txdata.length; i++) {
    let rt = await getDataFromSignature(txdata[i].signature);
    if (rt !== undefined) {
      result.push(rt);
    }
  }
  return result;
};

// Parse activity from a transaction siganture
export const getDataFromSignature = async (sig: string) => {
  // Get transaction data from on-chain
  let tx;
  try {
    tx = await solConnection.getParsedTransaction(sig, "confirmed");
  } catch (e) {}

  if (!tx) {
    console.log(`Can't get Transaction for ${sig}`);
    return;
  }

  if (tx.meta?.err !== null) {
    console.log(`Failed Transaction: ${sig}`);
    return;
  }

  // Parse activty by analyze fetched Transaction data
  let length = tx.transaction.message.instructions.length;
  let valid = 0;
  let hash = "";
  let ixId = -1;
  for (let i = 0; i < length; i++) {
    hash = (
      tx.transaction.message.instructions[i] as PartiallyDecodedInstruction
    ).data;
    if (hash !== undefined && hash.slice(0, 8) === NONCE) {
      valid = 1;
    }
    if (valid === 1) {
      ixId = i;
      break;
    }
  }

  if (ixId === -1 || valid === 0) {
    return;
  }

  let ts = tx.slot ?? 0;
  if (!tx.meta.innerInstructions) {
    console.log(`Can't parse innerInstructions ${sig}`);
    return;
  }

  let accountKeys = (
    tx.transaction.message.instructions[ixId] as PartiallyDecodedInstruction
  ).accounts;
  let signer = accountKeys[0].toBase58();

  let bytes = bs58.decode(hash);
  let a = bytes.slice(8, 16).reverse();
  let type = new anchor.BN(a).toNumber();
  let b = bytes.slice(16, 24).reverse();
  let sol_price = new anchor.BN(b).toNumber();

  let state = type === ts % 2 ? 1 : 0;

  let result = {
    type: type,
    address: signer,
    bet_amount: sol_price,
    block_hash: ts,
    win: state,
    signature: sig,
  };

  return result;
};

main();

