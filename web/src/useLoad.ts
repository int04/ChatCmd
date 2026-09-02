import { useCallback, useEffect, useRef, useState } from 'react';
import { tr } from './i18n';

export function useLoad<T>(loader: () => Promise<T>, dependencies: unknown[] = []) {
  const [data, setData] = useState<T>();
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const requestGenerationRef = useRef(0);

  const reload = useCallback(async () => {
    const generation = ++requestGenerationRef.current;
    setLoading(true);
    setError('');
    try {
      const result = await loader();
      if (generation !== requestGenerationRef.current) return;
      setData(result);
    } catch (value) {
      if (generation !== requestGenerationRef.current) return;
      setError(value instanceof Error ? value.message : tr('Unknown error'));
    } finally {
      if (generation === requestGenerationRef.current) setLoading(false);
    }
  }, dependencies); // eslint-disable-line react-hooks/exhaustive-deps

  const refresh = useCallback(async () => {
    const generation = ++requestGenerationRef.current;
    try {
      const result = await loader();
      if (generation !== requestGenerationRef.current) return;
      setData(result);
      setLoading(false);
      setError('');
    } catch {
      // Background refresh must preserve the currently visible data and error state.
    }
  }, dependencies); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    void reload();
    return () => { requestGenerationRef.current += 1; };
  }, [reload]);
  return { data, setData, error, loading, reload, refresh };
}
