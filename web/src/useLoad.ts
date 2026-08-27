import { useCallback, useEffect, useState } from 'react';

export function useLoad<T>(loader: () => Promise<T>, dependencies: unknown[] = []) {
  const [data, setData] = useState<T>(); const [error, setError] = useState(''); const [loading, setLoading] = useState(true);
  const reload = useCallback(async () => { setLoading(true); setError(''); try { setData(await loader()); } catch (value) { setError(value instanceof Error ? value.message : 'Unknown error'); } finally { setLoading(false); } }, dependencies); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { void reload(); }, [reload]);
  return { data, setData, error, loading, reload };
}
