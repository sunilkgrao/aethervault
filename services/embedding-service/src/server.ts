#!/usr/bin/env node
/**
 * Standalone embedding service using node-llama-cpp
 * Provides OpenAI-compatible API on port 11435
 * 
 * Optimizations from qmd:
 * - Inactivity timeout for context disposal (keeps model loaded)
 * - Concurrent load protection
 * - Activity tracking
 * - Batch processing
 * - Metrics endpoint
 */

import express from 'express';
import cors from 'cors';
import { getLlama, resolveModelFile, LlamaLogLevel, type Llama, type LlamaModel, type LlamaEmbeddingContext } from 'node-llama-cpp';
import { homedir } from 'os';
import { join } from 'path';

const app = express();
const HOST = process.env.HOST || '127.0.0.1';
const PORT = parseInt(process.env.PORT || '11435');
const MODEL_CACHE_DIR = process.env.MODEL_CACHE_DIR || join(homedir(), '.cache', 'embedding-service', 'models');
const INACTIVITY_TIMEOUT_MS = parseInt(process.env.INACTIVITY_TIMEOUT_MS || '300000'); // 5 minutes default

// Default to embeddinggemma-300M (same as qmd)
const EMBED_MODEL = process.env.EMBED_MODEL || 'hf:ggml-org/embeddinggemma-300M-GGUF/embeddinggemma-300M-Q8_0.gguf';

interface EmbeddingRequest {
  input: string | string[];
  model?: string;
  encoding_format?: 'float' | 'base64';
}

interface EmbeddingResponse {
  object: 'list';
  data: Array<{
    object: 'embedding';
    embedding: number[];
    index: number;
  }>;
  model: string;
  usage: {
    prompt_tokens: number;
    total_tokens: number;
  };
}

// Global state
let llama: Llama | null = null;
let model: LlamaModel | null = null;
let embedContext: LlamaEmbeddingContext | null = null;
let isInitialized = false;
let initPromise: Promise<void> | null = null;
let modelLoadPromise: Promise<LlamaModel> | null = null;
let contextCreatePromise: Promise<LlamaEmbeddingContext> | null = null;

// Activity tracking
let inactivityTimer: ReturnType<typeof setTimeout> | null = null;
let lastActivityTime = Date.now();

// Metrics
let metrics = {
  totalRequests: 0,
  totalEmbeddings: 0,
  errors: 0,
  averageLatencyMs: 0,
  modelLoads: 0,
  contextReloads: 0,
  startTime: Date.now(),
};

/**
 * Touch activity - reset inactivity timer
 */
function touchActivity(): void {
  lastActivityTime = Date.now();

  if (inactivityTimer) {
    clearTimeout(inactivityTimer);
  }

  if (INACTIVITY_TIMEOUT_MS > 0 && embedContext) {
    inactivityTimer = setTimeout(async () => {
      const idleTime = Date.now() - lastActivityTime;
      if (idleTime >= INACTIVITY_TIMEOUT_MS) {
        console.log('⏱️  Disposing context after inactivity timeout');
        await unloadIdleContext();
      }
    }, INACTIVITY_TIMEOUT_MS);
    
    // Don't block process exit
    inactivityTimer.unref();
  }
}

/**
 * Unload context to save memory (keep model loaded)
 */
async function unloadIdleContext(): Promise<void> {
  if (embedContext) {
    try {
      await embedContext.dispose();
      embedContext = null;
      contextCreatePromise = null;
      console.log('✅ Context disposed (model still loaded)');
    } catch (error) {
      console.error('❌ Error disposing context:', error);
    }
  }

  if (inactivityTimer) {
    clearTimeout(inactivityTimer);
    inactivityTimer = null;
  }
}

/**
 * Ensure llama instance exists
 */
async function ensureLlama(): Promise<Llama> {
  if (!llama) {
    llama = await getLlama({ logLevel: LlamaLogLevel.error });
  }
  return llama;
}

/**
 * Ensure model is loaded (with concurrent load protection)
 */
async function ensureModel(): Promise<LlamaModel> {
  if (model) {
    return model;
  }

  // Prevent concurrent loads
  if (modelLoadPromise) {
    return modelLoadPromise;
  }

  modelLoadPromise = (async () => {
    console.log('📥 Loading model...');
    const llamaInstance = await ensureLlama();
    
    const modelPath = await resolveModelFile(EMBED_MODEL, {
      directory: MODEL_CACHE_DIR,
    });

    const loadedModel = await llamaInstance.loadModel({ modelPath });
    model = loadedModel;
    metrics.modelLoads++;
    console.log('✅ Model loaded');
    return loadedModel;
  })();

  try {
    return await modelLoadPromise;
  } finally {
    modelLoadPromise = null;  // Clear so failures can retry
  }
}

/**
 * Ensure embedding context exists (with concurrent create protection)
 */
async function ensureEmbedContext(): Promise<LlamaEmbeddingContext> {
  if (embedContext) {
    return embedContext;
  }

  // Prevent concurrent context creation
  if (contextCreatePromise) {
    return contextCreatePromise;
  }

  contextCreatePromise = (async () => {
    console.log('🔧 Creating embedding context...');
    const modelInstance = await ensureModel();
    const context = await modelInstance.createEmbeddingContext();
    embedContext = context;
    metrics.contextReloads++;
    console.log('✅ Context created');
    return context;
  })();

  try {
    return await contextCreatePromise;
  } finally {
    contextCreatePromise = null;  // Clear so failures can retry
  }
}

/**
 * Initialize the embedding model
 */
async function initializeModel(): Promise<void> {
  if (initPromise) {
    return initPromise;
  }

  initPromise = (async () => {
    console.log('🚀 Initializing embedding service...');
    console.log(`📦 Model: ${EMBED_MODEL}`);
    console.log(`📁 Cache dir: ${MODEL_CACHE_DIR}`);
    console.log(`⏱️  Inactivity timeout: ${INACTIVITY_TIMEOUT_MS}ms`);

    try {
      // Pre-load model and context
      await ensureEmbedContext();
      isInitialized = true;
      touchActivity();
      console.log('✅ Embedding service initialized successfully');
    } catch (error) {
      console.error('❌ Failed to initialize:', error);
      throw error;
    }
  })();

  return initPromise;
}

/**
 * Generate embedding for a single text
 */
async function generateEmbedding(text: string): Promise<number[]> {
  touchActivity();
  const context = await ensureEmbedContext();
  const embedding = await context.getEmbeddingFor(text);
  return Array.from(embedding.vector);
}

/**
 * Batch generate embeddings (parallel processing)
 */
async function generateEmbeddings(texts: string[]): Promise<number[][]> {
  touchActivity();
  const context = await ensureEmbedContext();

  // Process in parallel - node-llama-cpp handles internal batching
  const embeddings = await Promise.all(
    texts.map(async (text) => {
      const embedding = await context.getEmbeddingFor(text);
      touchActivity(); // Keep alive during long batches
      return Array.from(embedding.vector);
    })
  );

  return embeddings;
}

// Middleware
app.use(cors());
app.use(express.json());

// Request logging middleware
app.use((req, res, next) => {
  const start = Date.now();
  res.on('finish', () => {
    const duration = Date.now() - start;
    console.log(`${req.method} ${req.path} - ${res.statusCode} (${duration}ms)`);
  });
  next();
});

// Health check endpoint
app.get('/health', (req, res) => {
  res.json({
    status: isInitialized ? 'ready' : 'initializing',
    model: EMBED_MODEL,
    uptime: Date.now() - metrics.startTime,
    lastActivity: Date.now() - lastActivityTime,
    contextLoaded: !!embedContext,
    modelLoaded: !!model,
  });
});

// Metrics endpoint
app.get('/metrics', (req, res) => {
  res.json({
    ...metrics,
    uptime: Date.now() - metrics.startTime,
    requestsPerSecond: metrics.totalRequests / ((Date.now() - metrics.startTime) / 1000),
    lastActivity: Date.now() - lastActivityTime,
    contextLoaded: !!embedContext,
    modelLoaded: !!model,
  });
});

// OpenAI-compatible embeddings endpoint
app.post('/v1/embeddings', async (req, res) => {
  const requestStart = Date.now();
  metrics.totalRequests++;

  try {
    // Ensure model is initialized
    if (!isInitialized) {
      await initializeModel();
    }

    const { input, model: requestedModel, encoding_format = 'float' }: EmbeddingRequest = req.body;

    if (!input) {
      return res.status(400).json({
        error: {
          message: 'Missing required parameter: input',
          type: 'invalid_request_error',
        },
      });
    }

    if (encoding_format === 'base64') {
      return res.status(400).json({
        error: {
          message: 'base64 encoding format not supported',
          type: 'invalid_request_error',
        },
      });
    }

    // Handle both single string and array of strings
    const inputs = Array.isArray(input) ? input : [input];
    metrics.totalEmbeddings += inputs.length;

    // Generate embeddings (batch if multiple)
    let embeddings: number[][];
    if (inputs.length === 1) {
      const embedding = await generateEmbedding(inputs[0]);
      embeddings = [embedding];
    } else {
      embeddings = await generateEmbeddings(inputs);
    }

    // Calculate token usage (rough estimate)
    const totalTokens = inputs.reduce((sum, text) => sum + Math.ceil(text.length / 4), 0);

    // Update average latency
    const latency = Date.now() - requestStart;
    metrics.averageLatencyMs = 
      (metrics.averageLatencyMs * (metrics.totalRequests - 1) + latency) / metrics.totalRequests;

    const response: EmbeddingResponse = {
      object: 'list',
      data: embeddings.map((embedding, index) => ({
        object: 'embedding' as const,
        embedding,
        index,
      })),
      model: requestedModel || EMBED_MODEL,
      usage: {
        prompt_tokens: totalTokens,
        total_tokens: totalTokens,
      },
    };

    res.json(response);
  } catch (error) {
    metrics.errors++;
    console.error('Error generating embeddings:', error);
    res.status(500).json({
      error: {
        message: error instanceof Error ? error.message : 'Internal server error',
        type: 'internal_error',
      },
    });
  }
});

// Start server
async function start() {
  // Pre-initialize model on startup
  await initializeModel();

  app.listen(PORT, HOST, () => {
    console.log(`🌐 Embedding service listening on http://${HOST}:${PORT}`);
    console.log(`📊 POST /v1/embeddings - OpenAI-compatible embedding endpoint`);
    console.log(`💚 GET /health - Health check`);
    console.log(`📈 GET /metrics - Service metrics`);
  });
}

// Graceful shutdown
async function shutdown() {
  console.log('🛑 Shutting down gracefully...');
  
  if (inactivityTimer) {
    clearTimeout(inactivityTimer);
  }

  if (embedContext) {
    await embedContext.dispose();
  }
  if (model) {
    await model.dispose();
  }
  if (llama) {
    await llama.dispose();
  }
  
  process.exit(0);
}

process.on('SIGTERM', shutdown);
process.on('SIGINT', shutdown);

start().catch((error) => {
  console.error('Failed to start server:', error);
  process.exit(1);
});
