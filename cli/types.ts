import * as anchor from "@coral-xyz/anchor";
import { PublicKey } from "@solana/web3.js";

export interface GlobalPool {
  superAdmin: PublicKey; // 32
  loyaltyWallet: PublicKey; // 8
  loyaltyFee: anchor.BN; // 8
  totalRound: anchor.BN; // 8
}

export interface AccountData {
  name: String;
  nftMint: PublicKey;
}

export interface GameData {
  playTime: anchor.BN; // 8
  amout: anchor.BN; // 8
  rewardAmount: anchor.BN; // 8
  setNum: anchor.BN; // 8
  rand: anchor.BN; // 8
}

export interface PlayerPool {
  // 8 + 104 = 112
  player: PublicKey; // 32
  round: anchor.BN; // 8
  gameData: GameData; // 40
  winTimes: anchor.BN; // 8
  receivedReward: anchor.BN; // 8
  claimableReward: anchor.BN; // 8
}

