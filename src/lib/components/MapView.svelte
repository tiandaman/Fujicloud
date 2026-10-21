<script lang="ts" module>
  // The camera survives leaving and coming back to Places.
  let lastView: { center: [number, number]; zoom: number } | null = null;
</script>

<script lang="ts">
  import type { Map as MapLibre, Marker, StyleSpecification } from 'maplibre-gl';
  import type Supercluster from 'supercluster';
  import { onMount } from 'svelte';
  import { api } from '../api';
  import { formatCount, plural } from '../format';
  import { placeName } from '../recipe';
  import { store } from '../store.svelte';
  import type { Photo } from '../types';
  import Icon from './Icon.svelte';

  type Props = { id: number; score: number };
  type ClusterProps = Props & { best: number };

  let container: HTMLDivElement;
  let map: MapLibre | null = null;
  let maplibre: typeof import('maplibre-gl') | null = null;
  let index = $state<Supercluster<Props, ClusterProps> | null>(null);
  let loading = $state(true);
  let failed = $state<string | null>(null);
  let drawer = $state<{ title: string; photos: Photo[] } | null>(null);
  let detailed = $state(readPref());
  const markers = new Map<string, Marker>();

  const located = $derived(store.visible.filter((p) => p.lat != null && p.lon != null));
  const placeCount = $derived(new Set(located.map((p) => placeName(p)).filter(Boolean)).size);

  function readPref() {
    try {
      return localStorage.getItem('fujicloud.map.detail') !== 'off';
    } catch {
      return true;
    }
  }

  function style(countries: GeoJSON.FeatureCollection, online: boolean): StyleSpecification {
    const layers: StyleSpecification['layers'] = [
      { id: 'ocean', type: 'background', paint: { 'background-color': '#0d1117' } },
      { id: 'land', type: 'fill', source: 'countries', paint: { 'fill-color': '#1c2028' } },
    ];
    if (online) {
      // OpenMapTiles schema, served by OpenFreeMap (free, no key).
      layers.push(
        { id: 'water', type: 'fill', source: 'omt', 'source-layer': 'water', paint: { 'fill-color': '#0d1117' } },
        { id: 'park', type: 'fill', source: 'omt', 'source-layer': 'park', minzoom: 8, paint: { 'fill-color': '#1d2a23', 'fill-opacity': 0.6 } },
        { id: 'landcover', type: 'fill', source: 'omt', 'source-layer': 'landcover', minzoom: 6, filter: ['in', ['get', 'class'], ['literal', ['wood', 'grass', 'farmland']]], paint: { 'fill-color': '#1e2620', 'fill-opacity': 0.45 } },
        { id: 'building', type: 'fill', source: 'omt', 'source-layer': 'building', minzoom: 14, paint: { 'fill-color': '#232831' } },
        {
          id: 'roads',
          type: 'line',
          source: 'omt',
          'source-layer': 'transportation',
          minzoom: 5,
          filter: ['in', ['get', 'class'], ['literal', ['motorway', 'trunk', 'primary', 'secondary', 'tertiary', 'minor', 'service']]],
          paint: {
            'line-color': ['match', ['get', 'class'], ['motorway', 'trunk'], '#3b4250', '#2b3039'],
            'line-width': ['interpolate', ['exponential', 1.6], ['zoom'], 5, 0.4, 12, 1.4, 16, 6],
          },
        },
      );
    }
    layers.push({
      id: 'borders',
      type: 'line',
      source: 'countries',
      paint: { 'line-color': '#363c48', 'line-width': ['interpolate', ['linear'], ['zoom'], 1, 0.4, 6, 1.1] },
    });
    if (online) {
      layers.push({
        id: 'labels',
        type: 'symbol',
        source: 'omt',
        'source-layer': 'place',
        minzoom: 2,
        filter: ['in', ['get', 'class'], ['literal', ['country', 'city', 'town', 'village', 'suburb']]],
        layout: {
          'text-field': ['coalesce', ['get', 'name:en'], ['get', 'name']],
          'text-font': ['Noto Sans Regular'],
          'text-size': ['match', ['get', 'class'], 'country', 12, 'city', 13, 11],
        },
        paint: { 'text-color': '#8a919e', 'text-halo-color': '#0d1117', 'text-halo-width': 1.4 },
      });
    }
    return {
      version: 8,
      projection: { type: 'globe' },
      glyphs: 'https://tiles.openfreemap.org/fonts/{fontstack}/{range}.pbf',
      sources: {
        countries: { type: 'geojson', data: countries },
        ...(online ? { omt: { type: 'vector' as const, url: 'https://tiles.openfreemap.org/planet' } } : {}),
      },
      layers,
    };
  }

  let countries: GeoJSON.FeatureCollection | null = null;

  onMount(() => {
    let cancelled = false;
    (async () => {
      try {
        const [lib, workerUrl, { default: SC }, topo, atlas] = await Promise.all([
          import('maplibre-gl'),
          import('maplibre-gl/dist/maplibre-gl-worker.mjs?url'),
          import('supercluster'),
          import('topojson-client'),
          import('world-atlas/countries-50m.json'),
          import('maplibre-gl/dist/maplibre-gl.css'),
        ]);
        if (cancelled) return;
        maplibre = lib;
        lib.setWorkerUrl(workerUrl.default);
        const world = atlas.default as unknown as Parameters<typeof topo.feature>[0];
        countries = topo.feature(world, world.objects.countries) as unknown as GeoJSON.FeatureCollection;
        index = new SC<Props, ClusterProps>({
          radius: 56,
          maxZoom: 17,
          map: (p) => ({ id: p.id, score: p.score, best: p.id }),
          reduce: (acc, p) => {
            if (p.score > acc.score) {
              acc.score = p.score;
              acc.best = p.best;
            }
          },
        });
        map = new lib.Map({
          container,
          style: style(countries, detailed),
          center: lastView?.center ?? [10, 25],
          zoom: lastView?.zoom ?? 1.4,
          attributionControl: false,
          renderWorldCopies: false,
          maxPitch: 0,
        });
        map.dragRotate.disable();
        map.touchZoomRotate.disableRotation();
        map.addControl(new lib.AttributionControl({ compact: true, customAttribution: detailed ? '© OpenStreetMap · OpenFreeMap' : 'Natural Earth' }), 'bottom-right');
        map.on('moveend', render);
        map.on('move', schedule);
        map.on('load', () => {
          loading = false;
          load();
          if (!lastView && !store.mapFocus) fitAll();
        });
      } catch (e) {
        failed = String(e);
        loading = false;
      }
    })();
    return () => {
      cancelled = true;
      if (map) lastView = { center: map.getCenter().toArray() as [number, number], zoom: map.getZoom() };
      markers.forEach((m) => m.remove());
      markers.clear();
      map?.remove();
      map = null;
    };
  });

  // Rebuild clusters whenever the visible photo set changes (filters, ratings).
  $effect(() => {
    void located;
    if (index && map) load();
  });

  // "Show on map" from elsewhere.
  $effect(() => {
    const focus = store.mapFocus;
    if (!focus || !map || loading) return;
    map.flyTo({ center: [focus.lon, focus.lat], zoom: focus.zoom ?? 12, duration: 1400, essential: true });
    store.mapFocus = null;
  });

  function load() {
    if (!index) return;
    // Cluster ids are reassigned on every load, so start the markers afresh.
    markers.forEach((m) => m.remove());
    markers.clear();
    index.load(
      located.map((p) => ({
        type: 'Feature' as const,
        geometry: { type: 'Point' as const, coordinates: [p.lon!, p.lat!] },
        // The cluster's cover photo: picks and high ratings first, then newest.
        properties: { id: p.id, score: p.rating * 1e11 + (p.flag === 1 ? 5e10 : 0) + (p.takenAt ?? 0) },
      })),
    );
    render();
  }

  function fitAll() {
    if (!map || !maplibre || !located.length) return;
    const bounds = new maplibre.LngLatBounds();
    for (const p of located) bounds.extend([p.lon!, p.lat!]);
    map.fitBounds(bounds, { padding: 80, maxZoom: 10, duration: 0 });
  }

  let frame = 0;
  function schedule() {
    cancelAnimationFrame(frame);
    frame = requestAnimationFrame(render);
  }

  function render() {
    if (!map || !index || !maplibre) return;
    const zoom = Math.round(map.getZoom());
    const b = map.getBounds();
    const bbox: [number, number, number, number] = zoom < 3 ? [-180, -85, 180, 85] : [b.getWest(), b.getSouth(), b.getEast(), b.getNorth()];
    const clusters = index.getClusters(bbox, zoom);
    const seen = new Set<string>();
    // On the globe, bubbles over the horizon must not float in space.
    const center = map.getCenter();
    const facing = (lon: number, lat: number) => {
      const r = Math.PI / 180;
      const cos =
        Math.sin(lat * r) * Math.sin(center.lat * r) + Math.cos(lat * r) * Math.cos(center.lat * r) * Math.cos((lon - center.lng) * r);
      return cos > 0.12;
    };
    for (const c of clusters) {
      const props = c.properties as Partial<ClusterProps> & { cluster?: boolean; cluster_id?: number; point_count?: number };
      const key = props.cluster ? `c${props.cluster_id}` : `p${props.id}`;
      const [lon, lat] = c.geometry.coordinates;
      if (map.getZoom() < 5 && !facing(lon, lat)) continue;
      seen.add(key);
      if (markers.has(key)) continue;
      const cover = store.byId.get(props.cluster ? props.best! : props.id!);
      if (!cover) continue;
      const el = document.createElement('button');
      el.className = props.cluster ? 'bubble cluster' : 'bubble';
      el.setAttribute('aria-label', props.cluster ? `${props.point_count} photos` : cover.name);
      const img = document.createElement('img');
      img.src = api.thumbUrl(cover);
      img.alt = '';
      img.draggable = false;
      el.appendChild(img);
      if (props.cluster) {
        const n = document.createElement('span');
        n.textContent = formatCount(props.point_count!);
        el.appendChild(n);
      }
      el.addEventListener('click', (e) => {
        e.stopPropagation();
        open(c.geometry.coordinates as [number, number], props.cluster ? props.cluster_id! : null, cover.id);
      });
      markers.set(key, new maplibre.Marker({ element: el, anchor: 'center', opacityWhenCovered: '0' }).setLngLat(c.geometry.coordinates as [number, number]).addTo(map));
    }
    for (const [key, m] of markers) {
      if (!seen.has(key)) {
        m.remove();
        markers.delete(key);
      }
    }
  }

  function open(center: [number, number], clusterId: number | null, id: number) {
    if (!index || !map) return;
    if (clusterId == null) {
      const p = store.byId.get(id)!;
      drawer = { title: placeName(p) ?? 'Photo', photos: [p] };
      return;
    }
    const ids = index.getLeaves(clusterId, Infinity).map((f) => f.properties.id);
    const photos = ids
      .map((i) => store.byId.get(i))
      .filter((p): p is Photo => !!p)
      .sort((a, b) => (b.takenAt ?? 0) - (a.takenAt ?? 0));
    // Title the panel after the most common place in it.
    const tally = new Map<string, number>();
    for (const p of photos) {
      const name = placeName(p);
      if (name) tally.set(name, (tally.get(name) ?? 0) + 1);
    }
    const top = [...tally].sort((a, b) => b[1] - a[1]);
    const title = top.length === 0 ? 'Photos' : top.length === 1 ? top[0][0] : `${top[0][0]} and nearby`;
    drawer = { title, photos };
    const zoom = index.getClusterExpansionZoom(clusterId);
    map.easeTo({ center, zoom: Math.min(zoom, 16), duration: 700 });
  }

  function toggleDetail() {
    detailed = !detailed;
    try {
      localStorage.setItem('fujicloud.map.detail', detailed ? 'on' : 'off');
    } catch {
      // Not persisted; fine.
    }
    if (map && countries) map.setStyle(style(countries, detailed));
  }
</script>

<div class="wrap">
  <div class="map" bind:this={container}></div>

  <div class="overlay top">
    <div class="stat">
      <Icon name="globe" size={15} />
      <span>{plural(located.length, 'photo')}{placeCount ? ` · ${plural(placeCount, 'place')}` : ''}</span>
    </div>
    <button class="chip" class:on={detailed} title="Streets and labels from OpenStreetMap (needs internet)" onclick={toggleDetail}>
      Street detail
    </button>
    <button class="chip" title="Fit all photos" onclick={fitAll}><Icon name="expand" size={13} /></button>
  </div>

  {#if loading}
    <div class="center faint">Loading map…</div>
  {:else if failed}
    <div class="center">
      <h3>The map couldn't load</h3>
      <p class="muted">{failed}</p>
    </div>
  {:else if !located.length}
    <div class="center card">
      <Icon name="pin" size={28} stroke={1.4} />
      <h3>No locations yet</h3>
      <p class="muted">
        Turn on <em>Location sync</em> in the FUJIFILM XApp (Bluetooth). Your phone then sends its position to the camera, and each shot
        gets GPS coordinates. Photos without GPS borrow the location of a geotagged shot from the same half hour.
      </p>
    </div>
  {/if}

  {#if drawer}
    <aside class="drawer">
      <header>
        <div>
          <h3 class="truncate">{drawer.title}</h3>
          <span class="faint">{plural(drawer.photos.length, 'photo')}</span>
        </div>
        <button class="icon-btn" aria-label="Close" onclick={() => (drawer = null)}><Icon name="x" size={15} /></button>
      </header>
      <div class="thumbs">
        {#each drawer.photos.slice(0, 300) as p (p.id)}
            <button
            class="thumb"
            aria-label={p.name}
            onclick={() =>
              store.open(
                p.id,
                drawer!.photos.map((x) => x.id),
              )}
          >
            <img src={api.thumbUrl(p)} alt="" loading="lazy" draggable="false" />
          </button>
        {/each}
      </div>
      <footer>
        <button
          class="btn"
          onclick={() =>
            store.setCollection({
              type: 'ids',
              ids: drawer!.photos.map((p) => p.id),
              title: drawer!.title,
            })}
        >
          Show in library
        </button>
      </footer>
    </aside>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
    flex: 1;
    min-width: 0;
    background: #07080a;
    overflow: hidden;
  }
  .map {
    position: absolute;
    inset: 0;
  }
  .overlay.top {
    position: absolute;
    top: 14px;
    left: 14px;
    display: flex;
    gap: 6px;
    z-index: 2;
  }
  .stat,
  .chip {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 12px;
    border-radius: 15px;
    background: rgba(28, 30, 36, 0.82);
    backdrop-filter: blur(14px) saturate(1.4);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.35), inset 0 0 0 0.5px var(--border);
    font-size: 12px;
    font-weight: 550;
  }
  .stat :global(svg) {
    color: var(--accent);
  }
  .chip {
    color: var(--text-2);
  }
  .chip:hover {
    color: var(--text);
  }
  .chip.on {
    color: var(--text);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.35), inset 0 0 0 1px var(--accent);
  }
  .center {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    text-align: center;
    z-index: 2;
  }
  .center.card {
    width: min(420px, 80%);
    padding: 22px 26px;
    border-radius: var(--radius-lg);
    background: rgba(28, 30, 36, 0.9);
    backdrop-filter: blur(16px);
    box-shadow: var(--shadow-md);
    color: var(--text-2);
  }
  .center h3 {
    margin: 10px 0 6px;
    color: var(--text);
    font: 600 15px var(--font-display);
  }
  .center p {
    margin: 0;
    line-height: 1.5;
  }
  em {
    font-style: normal;
    color: var(--text);
  }

  /* Map markers are created imperatively, so these styles are global. */
  .wrap :global(.bubble) {
    position: relative;
    width: 46px;
    height: 46px;
    padding: 0;
    border-radius: 12px;
    background: #2a2d34;
    box-shadow:
      0 0 0 2.5px #fff,
      0 6px 18px rgba(0, 0, 0, 0.55);
    cursor: pointer;
    transition: transform 160ms var(--ease-out);
  }
  .wrap :global(.bubble.cluster) {
    width: 56px;
    height: 56px;
    border-radius: 14px;
  }
  .wrap :global(.bubble.maplibregl-marker-covered) {
    pointer-events: none;
  }
  .wrap :global(.bubble:hover) {
    transform: scale(1.1);
    z-index: 5;
  }
  .wrap :global(.bubble img) {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: inherit;
    display: block;
  }
  .wrap :global(.bubble span) {
    position: absolute;
    top: -8px;
    right: -10px;
    min-width: 22px;
    height: 22px;
    padding: 0 6px;
    border-radius: 11px;
    background: var(--accent);
    color: white;
    font: 650 11.5px/22px var(--font);
    box-shadow: 0 0 0 2px #0d1117;
  }
  .wrap :global(.maplibregl-ctrl-attrib) {
    background: rgba(13, 17, 23, 0.7) !important;
    color: var(--text-3);
    font-size: 10px;
  }
  .wrap :global(.maplibregl-ctrl-attrib a) {
    color: var(--text-2);
  }

  .drawer {
    position: absolute;
    top: 12px;
    right: 12px;
    bottom: 12px;
    width: 320px;
    display: flex;
    flex-direction: column;
    border-radius: var(--radius-lg);
    background: rgba(26, 28, 33, 0.94);
    backdrop-filter: blur(24px) saturate(1.4);
    box-shadow: var(--shadow-lg);
    z-index: 3;
    animation: slide 240ms var(--ease-out);
  }
  @keyframes slide {
    from {
      opacity: 0;
      transform: translateX(16px);
    }
  }
  .drawer header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 8px;
    padding: 14px 10px 10px 16px;
  }
  .drawer header div {
    min-width: 0;
  }
  .drawer h3 {
    margin: 0 0 2px;
    font: 600 15px var(--font-display);
  }
  .drawer header span {
    font-size: 12px;
  }
  .thumbs {
    flex: 1;
    overflow-y: auto;
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 3px;
    padding: 0 12px 12px;
    align-content: start;
  }
  .thumb {
    position: relative;
    display: block;
    width: 100%;
    padding-top: 100%;
    border-radius: 4px;
    overflow: hidden;
    background: var(--tile);
  }
  .thumb img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    transition: transform 200ms var(--ease-out);
  }
  .thumb:hover img {
    transform: scale(1.06);
  }
  .drawer footer {
    padding: 10px 12px 12px;
    border-top: 1px solid var(--sep);
  }
  .drawer footer .btn {
    width: 100%;
  }
</style>
