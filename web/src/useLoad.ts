import { useCallback, useEffect, useRef, useState } from 'react';
import { tr } from './i18n';

export function useLoad<T>(loader: () => Promise<T>, dependencies: unknown[] = []) {
  const [data, setData] = useState<T>();
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const refreshingRef = useRef(false);

  const reload = useCallback(async () => {
    setLoading(true);
    setError('');
    try {
      setData(await loader());
    } catch (value) {
      setError(value instanceof Error ? value.message : tr('Unknown error'));
    } finally {
      setLoading(false);
    }
  }, dependencies); // eslint-disable-line react-hooks/exhaustive-deps

  const refresh = useCallback(async () => {
    if (refreshingRef.current) return;
    refreshingRef.current = true;
    try {
      setData(await loader());
      setError('');
    } catch {
      // Background refresh must preserve the currently visible data and error state.
    } finally {
      refreshingRef.current = false;
    }
  }, dependencies); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => { void reload(); }, [reload]);
  return { data, setData, error, loading, reload, refresh };
}
