import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { PublicKey, Connection, clusterApiUrl } from "@solana/web3.js";
import fs from "fs";
import path from "path";

const PROGRAM_ID   = new PublicKey("5g9zWHF1Vv6GiGpA2ZbJQbSCDZd5hAk9AyvabRJvKFx2");
const TREASURY     = new PublicKey("AnbjcK3uD5KYFtb3EuUxHTyJMfC4oyLo7hF2uELfKagN");
const KEYPAIR_PATH = path.join(process.env.HOME!, ".config/solana/id.json");

async function main() {
  // Load wallet
  const keypair = anchor.web3.Keypair.fromSecretKey(
    Uint8Array.from(JSON.parse(fs.readFileSync(KEYPAIR_PATH, "utf-8")))
  );
  console.log("Admin wallet:", keypair.publicKey.toBase58());

  // Set up provider
  const connection = new Connection(clusterApiUrl("devnet"), "confirmed");
  const wallet     = new anchor.Wallet(keypair);
  const provider   = new anchor.AnchorProvider(connection, wallet, { commitment: "confirmed" });
  anchor.setProvider(provider);

  // Load IDL
  const idl = JSON.parse(
    fs.readFileSync(
      path.join(__dirname, "../target/idl/aifinpay_contract.json"),
      "utf-8"
    )
  );

  const program = new Program(idl, provider);

  // Derive Vault PDA
  const [vaultPda] = PublicKey.findProgramAddressSync([Buffer.from("vault")], PROGRAM_ID);
  console.log("Vault PDA:", vaultPda.toBase58());

  // Check if already initialized
  const vaultAccount = await connection.getAccountInfo(vaultPda);
  if (vaultAccount) {
    console.log("Vault already initialized. Done.");
    return;
  }

  // Call initialize
  console.log("Initializing vault with treasury:", TREASURY.toBase58());
  const tx = await (program.methods as any)
    .initialize(TREASURY)
    .accounts({
      vault:         vaultPda,
      admin:         keypair.publicKey,
      systemProgram: anchor.web3.SystemProgram.programId,
    })
    .signers([keypair])
    .rpc();

  console.log("✅ Vault initialized! Transaction:", tx);
  console.log("   Solscan: https://solscan.io/tx/" + tx + "?cluster=devnet");
}

main().catch(console.error);
