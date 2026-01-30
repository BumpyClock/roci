import { spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

type ParsedArgs = {
  promptPathOverride?: string;
  maxIterationsOverride?: number;
};

const parseMaxIterations = (raw: string, flag: string): number => {
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error(`${flag} must be a positive integer`);
  }
  return parsed;
};

const parseArgs = (argv: string[]): ParsedArgs => {
  let promptPathOverride: string | undefined;
  let maxIterationsOverride: number | undefined;
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index] ?? '';
    if (arg === '--prompt') {
      const next = argv[index + 1];
      if (!next) {
        throw new Error(`${arg} requires a path`);
      }
      promptPathOverride = next;
      index += 1;
      continue;
    }
    if (arg.startsWith('--prompt=')) {
      const [, rawValue] = arg.split('=', 2);
      if (!rawValue) {
        throw new Error(`${arg} requires a path`);
      }
      promptPathOverride = rawValue;
      continue;
    }
    if (arg === '--max-iterations') {
      const next = argv[index + 1];
      if (!next) {
        throw new Error(`${arg} requires a number`);
      }
      maxIterationsOverride = parseMaxIterations(next, arg);
      index += 1;
      continue;
    }
    if (arg.startsWith('--max-iterations=')) {
      const [, rawValue] = arg.split('=', 2);
      if (!rawValue) {
        throw new Error(`${arg} requires a number`);
      }
      maxIterationsOverride = parseMaxIterations(rawValue, '--max-iterations');
    }
  }
  return { promptPathOverride, maxIterationsOverride };
};

const { promptPathOverride, maxIterationsOverride } = parseArgs(process.argv.slice(2));
const promptPath = resolve(process.cwd(), promptPathOverride ?? 'prompt.md');
const defaultMaxIterations = 10;
const maxIterations = maxIterationsOverride ?? defaultMaxIterations;
const defaultDelayMs = 1000;
const delayEnv = process.env.CLAUDE_LOOP_DELAY_MS;
const parsedDelay = delayEnv ? Number.parseInt(delayEnv, 10) : defaultDelayMs;
const delayMs = Number.isFinite(parsedDelay) && parsedDelay >= 0 ? parsedDelay : defaultDelayMs;

const readPrompt = (): string => readFileSync(promptPath, 'utf8');

const runClaude = async (prompt: string): Promise<string> =>
  await new Promise((resolve, reject) => {
    const child = spawn('claude', ['-p', prompt], { stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    if (child.stdout) {
      child.stdout.setEncoding('utf8');
      child.stdout.on('data', (chunk) => {
        stdout += chunk;
        process.stdout.write(chunk);
      });
    }
    if (child.stderr) {
      child.stderr.setEncoding('utf8');
      child.stderr.on('data', (chunk) => {
        stderr += chunk;
        process.stderr.write(chunk);
      });
    }
    child.on('error', (error) => {
      reject(error);
    });
    child.on('close', (code) => {
      if (code && code !== 0) {
        reject(new Error(`claude exited with code ${code}`));
        return;
      }
      resolve(`${stdout}${stderr}`);
    });
  });

const startSpinner = (label: string): (() => void) => {
  const frames = ['-', '\\', '|', '/'];
  let index = 0;
  const render = () => {
    const frame = frames[index % frames.length] ?? '-';
    index += 1;
    process.stderr.write(`\r${frame} ${label}`);
  };
  render();
  const timer = setInterval(render, 80);
  return () => {
    clearInterval(timer);
    const padding = ' '.repeat(label.length + 2);
    process.stderr.write(`\r${padding}\r${label} done\n`);
  };
};

const sleep = async (ms: number): Promise<void> => {
  if (ms <= 0) {
    return;
  }
  await new Promise<void>((resolve) => setTimeout(resolve, ms));
};

const main = async (): Promise<void> => {
  for (let iteration = 0; iteration < maxIterations; iteration += 1) {
    const label = `Iteration ${iteration + 1} of ${maxIterations}`;
    const stopSpinner = startSpinner(label);
    const output = await runClaude(readPrompt());
    stopSpinner();
    if (output.trim() === 'Task Complete') {
      return;
    }
    if (iteration < maxIterations - 1) {
      await sleep(delayMs);
    }
  }
  throw new Error(`Reached max iterations (${maxIterations}) without Task Complete`);
};

void main().catch((error) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(message);
  process.exit(1);
});
