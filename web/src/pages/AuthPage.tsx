import { Eye, EyeOff, LockKeyhole, Mail, ShieldCheck } from 'lucide-react';
import { useState, type FormEvent } from 'react';
import { Link, Navigate, useLocation, useNavigate } from 'react-router-dom';
import { ApiError } from '../api';
import { useAuth } from '../auth';

type Mode = 'login' | 'register';

export function AuthPage({ mode }: { mode: Mode }) {
  const { user, loading, login, register } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!loading && user) return <Navigate replace to="/" />;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const normalizedEmail = email.trim();
    if (!normalizedEmail) return setError('Vui lòng nhập email.');
    if (password.length < 8) return setError('Mật khẩu phải có ít nhất 8 ký tự.');
    if (mode === 'register' && password !== confirmPassword) return setError('Mật khẩu xác nhận không khớp.');
    setSubmitting(true);
    try {
      if (mode === 'login') await login(normalizedEmail, password);
      else await register(normalizedEmail, password);
      const from = (location.state as { from?: string } | null)?.from;
      navigate(from && from.startsWith('/') && from !== '/login' && from !== '/register' ? from : '/', { replace: true });
    } catch (value) {
      setError(value instanceof ApiError ? value.message : 'Không thể kết nối tới máy chủ.');
    } finally {
      setSubmitting(false);
    }
  };

  return <main className="auth-screen">
    <section className="auth-card" aria-labelledby="auth-title">
      <div className="auth-brand"><span className="auth-brand-icon"><ShieldCheck /></span><div><strong>ChatCMD</strong><span>Secure local agent workspace</span></div></div>
      <div className="auth-heading"><h1 id="auth-title">{mode === 'login' ? 'Đăng nhập' : 'Tạo tài khoản'}</h1><p>{mode === 'login' ? 'Đăng nhập để tiếp tục quản lý agent trên máy này.' : 'Tạo tài khoản ChatCMD để bắt đầu sử dụng ứng dụng.'}</p></div>
      <form className="auth-form" onSubmit={submit}>
        <label><span>Email</span><div className="auth-input"><Mail /><input autoFocus autoComplete="email" inputMode="email" type="email" value={email} onChange={(event) => setEmail(event.target.value)} placeholder="you@example.com" /></div></label>
        <label><span>Mật khẩu</span><div className="auth-input"><LockKeyhole /><input autoComplete={mode === 'login' ? 'current-password' : 'new-password'} type={showPassword ? 'text' : 'password'} value={password} onChange={(event) => setPassword(event.target.value)} placeholder="Tối thiểu 8 ký tự" /><button type="button" className="auth-password-toggle" aria-label={showPassword ? 'Ẩn mật khẩu' : 'Hiện mật khẩu'} onClick={() => setShowPassword((value) => !value)}>{showPassword ? <EyeOff /> : <Eye />}</button></div></label>
        {mode === 'register' && <label><span>Xác nhận mật khẩu</span><div className="auth-input"><LockKeyhole /><input autoComplete="new-password" type={showPassword ? 'text' : 'password'} value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} placeholder="Nhập lại mật khẩu" /></div></label>}
        {error && <div className="auth-error" role="alert">{error}</div>}
        <button className="button primary auth-submit" disabled={submitting || loading} type="submit">{submitting ? 'Đang xử lý…' : mode === 'login' ? 'Đăng nhập' : 'Đăng ký'}</button>
      </form>
      <p className="auth-switch">{mode === 'login' ? <>Chưa có tài khoản? <Link to="/register">Đăng ký</Link></> : <>Đã có tài khoản? <Link to="/login">Đăng nhập</Link></>}</p>
    </section>
  </main>;
}
