<script lang="ts">
  import { api } from '../api';
  import type { Photo } from '../types';

  let { photo }: { photo: Photo } = $props();
  let canvas: HTMLCanvasElement;
  let empty = $state(false);

  const W = 256;
  const H = 72;

  $effect(() => {
    const p = photo;
    const img = new Image();
    img.crossOrigin = 'anonymous';
    img.decoding = 'async';
    let cancelled = false;
    img.onload = () => {
      if (cancelled) return;
      empty = !draw(img);
    };
    img.onerror = () => (empty = true);
    img.src = api.thumbUrl(p);
    return () => (cancelled = true);
  });

  function draw(img: HTMLImageElement): boolean {
    const scale = Math.min(1, 200 / Math.max(img.naturalWidth, img.naturalHeight));
    const w = Math.max(1, Math.round(img.naturalWidth * scale));
    const h = Math.max(1, Math.round(img.naturalHeight * scale));
    const off = new OffscreenCanvas(w, h);
    const octx = off.getContext('2d', { willReadFrequently: true })!;
    octx.drawImage(img, 0, 0, w, h);
    let data: Uint8ClampedArray;
    try {
      data = octx.getImageData(0, 0, w, h).data;
    } catch {
      return false;
    }

    const bins = [new Float32Array(256), new Float32Array(256), new Float32Array(256)];
    for (let i = 0; i < data.length; i += 4) {
      bins[0][data[i]]++;
      bins[1][data[i + 1]]++;
      bins[2][data[i + 2]]++;
    }
    // Light smoothing, and ignore clipped ends when scaling so a blown sky
    // doesn't flatten everything else.
    const smooth = bins.map((b) => b.map((_, i) => (b[Math.max(0, i - 1)] + 2 * b[i] + b[Math.min(255, i + 1)]) / 4));
    let max = 1;
    for (const b of smooth) for (let i = 2; i < 254; i++) max = Math.max(max, b[i]);

    const ctx = canvas.getContext('2d')!;
    ctx.clearRect(0, 0, W, H);
    ctx.globalCompositeOperation = 'screen';
    const colors = ['rgba(255, 69, 58, 0.85)', 'rgba(48, 209, 88, 0.8)', 'rgba(10, 132, 255, 0.9)'];
    smooth.forEach((b, c) => {
      ctx.beginPath();
      ctx.moveTo(0, H);
      for (let i = 0; i < 256; i++) ctx.lineTo(i, H - Math.min(1, b[i] / max) * (H - 2));
      ctx.lineTo(255, H);
      ctx.closePath();
      ctx.fillStyle = colors[c];
      ctx.fill();
    });
    return true;
  }
</script>

<div class="hist" class:empty>
  <canvas bind:this={canvas} width={W} height={H}></canvas>
</div>

<style>
  .hist {
    position: relative;
    height: 72px;
    border-radius: 8px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.035);
    box-shadow: inset 0 0 0 0.5px var(--sep);
  }
  .hist::before {
    content: '';
    position: absolute;
    inset: 0;
    background: repeating-linear-gradient(to right, transparent 0 calc(25% - 1px), rgba(255, 255, 255, 0.05) calc(25% - 1px) 25%);
  }
  canvas {
    position: relative;
    width: 100%;
    height: 100%;
    display: block;
  }
  .empty canvas {
    opacity: 0;
  }
</style>
