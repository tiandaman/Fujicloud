/**
 * Part of every image URL. "Rebuild thumbnails" bumps it so the window
 * fetches the new renditions instead of reusing the images it has cached.
 */
export const cacheVersion = $state({ n: 0 });
