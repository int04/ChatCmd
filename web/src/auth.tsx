import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { api, ApiError } from './api';
import type { AuthInfo } from './types';

type AuthState = {
  user: AuthInfo | null;
  loading: boolean;
  refresh: () => Promise<AuthInfo | null>;
  login: (email: string, password: string) => Promise<AuthInfo>;
  register: (email: string, password: string) => Promise<AuthInfo>;
  logout: () => Promise<void>;
};

const AuthContext = createContext<AuthState | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [user, setUser] = useState<AuthInfo | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const info = await api.authInfo();
      setUser(info);
      return info;
    } catch (error) {
      if (error instanceof ApiError && error.status === 401) {
        setUser(null);
        return null;
      }
      throw error;
    }
  }, []);

  useEffect(() => {
    let active = true;
    void refresh()
      .catch(() => { if (active) setUser(null); })
      .finally(() => { if (active) setLoading(false); });
    const authRequired = () => setUser(null);
    window.addEventListener('chatcmd:auth-required', authRequired);
    return () => {
      active = false;
      window.removeEventListener('chatcmd:auth-required', authRequired);
    };
  }, [refresh]);

  const login = useCallback(async (email: string, password: string) => {
    await api.login(email, password);
    const info = await api.authInfo();
    setUser(info);
    return info;
  }, []);

  const register = useCallback(async (email: string, password: string) => {
    await api.register(email, password);
    const info = await api.authInfo();
    setUser(info);
    return info;
  }, []);

  const logout = useCallback(async () => {
    try { await api.logout(); } finally { setUser(null); }
  }, []);

  const value = useMemo<AuthState>(() => ({ user, loading, refresh, login, register, logout }), [user, loading, refresh, login, register, logout]);
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const value = useContext(AuthContext);
  if (!value) throw new Error('useAuth must be used inside AuthProvider');
  return value;
}
