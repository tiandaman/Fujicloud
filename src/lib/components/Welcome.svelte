<script lang="ts">
  import { api } from '../api';
  import { store } from '../store.svelte';
  import Icon from './Icon.svelte';

  let path = $state('');

  async function browse() {
    const p = await api.pickFolder('Choose your photo library');
    if (p) store.addSource(p);
  }
</script>

<div class="welcome">
  <img class="logo" src="/icon.svg" alt="" width="84" height="84" />
  <h1>Welcome to Fujicloud</h1>
  <p class="lede">Your whole Fujifilm library in one fast place: the Shield's SSD, this laptop, and every new shot straight from the camera.</p>

  <div class="steps">
    <div class="step">
      <span class="n">1</span>
      <div>
        <h3>Add your library</h3>
        <p>Point at the Shield's shared drive (e.g. <code>\\SHIELD\Samsung_T7</code>) or any folder.</p>
        <div class="inline">
          <input class="field" placeholder="\\SHIELD\Samsung_T7\Photos" bind:value={path} onkeydown={(e) => e.key === 'Enter' && path.trim() && store.addSource(path.trim())} />
          <button class="btn primary" onclick={() => (path.trim() ? store.addSource(path.trim()) : browse())}>
            {path.trim() ? 'Add' : 'Browse…'}
          </button>
        </div>
      </div>
    </div>
    <div class="step">
      <span class="n">2</span>
      <div>
        <h3>Turn on camera sync</h3>
        <p>New shots arriving over Wi-Fi are copied to the Shield and appear here automatically.</p>
        <button class="btn" onclick={() => (store.settingsOpen = true)}><Icon name="wifi" size={15} /> Set up sync</button>
      </div>
    </div>
  </div>
  <p class="drop faint"><Icon name="folder" size={14} /> Tip: you can also drag a folder onto this window.</p>
</div>

<style>
  .welcome {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 40px;
    text-align: center;
    animation: in 500ms var(--ease-out);
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
  }
  .logo {
    filter: drop-shadow(0 12px 30px rgba(255, 94, 98, 0.25));
  }
  h1 {
    margin: 18px 0 6px;
    font: 700 26px var(--font-display);
    letter-spacing: -0.02em;
  }
  .lede {
    max-width: 480px;
    margin: 0 0 28px;
    color: var(--text-2);
    font-size: 14px;
    line-height: 1.5;
  }
  .steps {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 14px;
    max-width: 760px;
    width: 100%;
    text-align: left;
  }
  .step {
    display: flex;
    gap: 14px;
    padding: 18px;
    border-radius: var(--radius-lg);
    background: rgba(255, 255, 255, 0.035);
    box-shadow: inset 0 0 0 0.5px var(--border);
  }
  .n {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 50%;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
    flex: none;
  }
  h3 {
    margin: 2px 0 4px;
    font: 600 14px var(--font-display);
  }
  .step p {
    margin: 0 0 12px;
    color: var(--text-2);
    line-height: 1.5;
  }
  code {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text);
  }
  .inline {
    display: flex;
    gap: 8px;
  }
  .inline .field {
    flex: 1;
  }
  .drop {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 22px;
  }
</style>
