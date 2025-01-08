import { expect } from "chai";
import { ethers } from "hardhat";
import { AppAuth } from "../typechain-types";
import { SignerWithAddress } from "@nomicfoundation/hardhat-ethers/signers";

describe("AppAuth", function () {
  let appAuth: AppAuth;
  let owner: SignerWithAddress;
  let user: SignerWithAddress;
  let appId: string;
  
  beforeEach(async function () {
    [owner, user] = await ethers.getSigners();
    appId = ethers.Wallet.createRandom().address;
    
    const AppAuth = await ethers.getContractFactory("AppAuth");
    appAuth = await AppAuth.deploy(appId);
    await appAuth.waitForDeployment();
  });

  describe("Basic functionality", function () {
    it("Should set the correct owner", async function () {
      expect(await appAuth.owner()).to.equal(owner.address);
    });

    it("Should set the correct app ID", async function () {
      expect(await appAuth.appId()).to.equal(appId);
    });
  });

  describe("Compose hash management", function () {
    const testHash = ethers.randomBytes(32);

    it("Should allow adding compose hash", async function () {
      await appAuth.addComposeHash(testHash);
      expect(await appAuth.allowedComposeHashes(testHash)).to.be.true;
    });

    it("Should allow removing compose hash", async function () {
      await appAuth.addComposeHash(testHash);
      await appAuth.removeComposeHash(testHash);
      expect(await appAuth.allowedComposeHashes(testHash)).to.be.false;
    });

    it("Should emit event when adding compose hash", async function () {
      await expect(appAuth.addComposeHash(testHash))
        .to.emit(appAuth, "ComposeHashAdded")
        .withArgs(testHash);
    });

    it("Should emit event when removing compose hash", async function () {
      await appAuth.addComposeHash(testHash);
      await expect(appAuth.removeComposeHash(testHash))
        .to.emit(appAuth, "ComposeHashRemoved")
        .withArgs(testHash);
    });
  });

  describe("isAppAllowed", function () {
    const composeHash = ethers.randomBytes(32);
    const deviceId = ethers.randomBytes(32);
    const mrEnclave = ethers.randomBytes(32);
    const mrImage = ethers.randomBytes(32);
    const instanceId = ethers.Wallet.createRandom().address;

    beforeEach(async function () {
      await appAuth.addComposeHash(composeHash);
    });

    it("Should allow valid boot info", async function () {
      const bootInfo = {
        appId: appId,
        composeHash: composeHash,
        instanceId: instanceId,
        deviceId: deviceId,
        mrEnclave: mrEnclave,
        mrImage: mrImage
      };

      const [isAllowed, reason] = await appAuth.isAppAllowed(bootInfo);
      expect(reason).to.equal("");
      expect(isAllowed).to.be.true;
    });

    it("Should reject invalid app ID", async function () {
      const bootInfo = {
        appId: ethers.Wallet.createRandom().address,
        composeHash: composeHash,
        instanceId: instanceId,
        deviceId: deviceId,
        mrEnclave: mrEnclave,
        mrImage: mrImage
      };

      const [isAllowed, reason] = await appAuth.isAppAllowed(bootInfo);
      expect(isAllowed).to.be.false;
      expect(reason).to.equal("Invalid app ID");
    });

    it("Should reject unallowed compose hash", async function () {
      const bootInfo = {
        appId: appId,
        composeHash: ethers.randomBytes(32),
        instanceId: instanceId,
        deviceId: deviceId,
        mrEnclave: mrEnclave,
        mrImage: mrImage
      };

      const [isAllowed, reason] = await appAuth.isAppAllowed(bootInfo);
      expect(isAllowed).to.be.false;
      expect(reason).to.equal("Compose hash not allowed");
    });
  });

  describe("Access control", function () {
    const testHash = ethers.randomBytes(32);

    it("Should prevent non-owners from adding compose hash", async function () {
      await expect(
        appAuth.connect(user).addComposeHash(testHash)
      ).to.be.revertedWith("Only owner can call this function");
    });

    it("Should prevent non-owners from removing compose hash", async function () {
      await appAuth.addComposeHash(testHash);
      await expect(
        appAuth.connect(user).removeComposeHash(testHash)
      ).to.be.revertedWith("Only owner can call this function");
    });
  });
});
