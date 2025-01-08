import fastify, { FastifyInstance } from 'fastify';
import { EthereumBackend } from './ethereum';
import { BootInfo, BootResponse } from './types';

declare module 'fastify' {
  interface FastifyInstance {
    ethereum: EthereumBackend;
  }
}

export async function build(): Promise<FastifyInstance> {
  const server = fastify({
    logger: true
  });

  // Register schema for request/response validation
  server.addSchema({
    $id: 'bootInfo',
    type: 'object',
    required: ['mrEnclave', 'mrImage', 'appId', 'composeHash', 'instanceId', 'deviceId'],
    properties: {
      mrEnclave: { type: 'string', description: 'MR Enclave measurement' },
      mrImage: { type: 'string', description: 'MR Image measurement' },
      appId: { type: 'string', description: 'Application ID' },
      composeHash: { type: 'string', description: 'Compose hash' },
      instanceId: { type: 'string', description: 'Instance ID' },
      deviceId: { type: 'string', description: 'Device ID' }
    }
  });

  server.addSchema({
    $id: 'bootResponse',
    type: 'object',
    required: ['isAllowed', 'reason'],
    properties: {
      isAllowed: { type: 'boolean' },
      reason: { type: 'string' }
    }
  });

  // Initialize backend
  const rpcUrl = process.env.ETH_RPC_URL || 'http://localhost:8545';
  const kmsContractAddr = process.env.KMS_CONTRACT_ADDR || '0x0000000000000000000000000000000000000000';
  server.decorate('ethereum', new EthereumBackend(rpcUrl, kmsContractAddr));

  // Define routes
  server.post<{
    Body: BootInfo;
    Reply: BootResponse;
  }>('/bootAuth/app', {
    schema: {
      body: { $ref: 'bootInfo#' },
      response: {
        200: { $ref: 'bootResponse#' }
      }
    }
  }, async (request, reply) => {
    try {
      return await server.ethereum.checkBoot(request.body, false);
    } catch (error) {
      reply.code(500).send({
        isAllowed: false,
        reason: `Error: ${error instanceof Error ? error.message : String(error)}`
      });
    }
  });

  server.post<{
    Body: BootInfo;
    Reply: BootResponse;
  }>('/bootAuth/kms', {
    schema: {
      body: { $ref: 'bootInfo#' },
      response: {
        200: { $ref: 'bootResponse#' }
      }
    }
  }, async (request, reply) => {
    try {
      return await server.ethereum.checkBoot(request.body, true);
    } catch (error) {
      reply.code(500).send({
        isAllowed: false,
        reason: `Error: ${error instanceof Error ? error.message : String(error)}`
      });
    }
  });

  return server;
}
