import './AccountUsage.css';
import { BarChart3, RefreshCw } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { api, type SkillUsageRow, type StatisticRow } from '../api';
import { tr } from '../i18n';

type Metric = 'countTurn' | 'countConversion' | 'countAgent' | 'countToolUse' | 'countSkill';

const METRICS: Array<{ key: Metric; label: string }> = [
  { key: 'countTurn', label: 'count_turn' },
  { key: 'countConversion', label: 'count_conversion' },
  { key: 'countAgent', label: 'count_agent' },
  { key: 'countToolUse', label: 'count_tool_use' },
  { key: 'countSkill', label: 'count_skill' },
];

export function AccountUsage() {
  const [metric, setMetric] = useState<Metric>('countTurn');
  const [statistics, setStatistics] = useState<StatisticRow[]>([]);
  const [skills, setSkills] = useState<SkillUsageRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [problem, setProblem] = useState('');

  const load = async () => {
    setLoading(true);
    setProblem('');
    try {
      const [statisticRows, skillRows] = await Promise.all([api.currentMonthStatistics(), api.skillUsage()]);
      setStatistics(statisticRows);
      setSkills(skillRows);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : tr('Could not load usage statistics.'));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { void load(); }, []);

  const days = useMemo(() => buildMonthDays(statistics, metric), [statistics, metric]);
  const total = useMemo(() => days.reduce((sum, day) => sum + day.value, 0), [days]);
  const max = useMemo(() => Math.max(0, ...days.map((day) => day.value)), [days]);
  const monthLabel = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric' }).format(new Date());

  return <section className="account-usage" aria-labelledby="account-usage-title">
    <div className="account-usage-heading">
      <div><span><BarChart3 /></span><div><strong id="account-usage-title">{tr('Usage this month')}</strong><small>{monthLabel}</small></div></div>
      <button type="button" onClick={() => void load()} disabled={loading} aria-label={tr('Refresh usage')}><RefreshCw className={loading ? 'spin' : ''} />{tr('Refresh')}</button>
    </div>

    <div className="account-usage-metrics" role="radiogroup" aria-label={tr('Usage metric')}>
      {METRICS.map((item) => <button key={item.key} type="button" role="radio" aria-checked={metric === item.key} className={metric === item.key ? 'active' : ''} onClick={() => setMetric(item.key)}>{item.label}</button>)}
    </div>

    {problem ? <div className="account-usage-error" role="alert">{problem}</div> : <>
      <div className="account-usage-summary"><span>{tr('Total')}</span><strong>{total.toLocaleString()}</strong></div>
      <div className="account-heatmap-wrap">
        <div className="account-heatmap-weekdays" aria-hidden="true">{['S', 'M', 'T', 'W', 'T', 'F', 'S'].map((day, index) => <span key={`${day}-${index}`}>{day}</span>)}</div>
        <div className="account-heatmap" aria-label={tr('Daily usage heatmap')}>
          {days.map((day) => <span key={day.key} className={`level-${heatLevel(day.value, max)}`} style={day.offset ? { gridColumnStart: day.offset + 1 } : undefined} title={`${day.label}: ${day.value.toLocaleString()}`} aria-label={`${day.label}: ${day.value.toLocaleString()}`} />)}
        </div>
        <div className="account-heatmap-legend"><span>{tr('Less')}</span>{[0, 1, 2, 3, 4].map((level) => <i key={level} className={`level-${level}`} />)}<span>{tr('More')}</span></div>
      </div>

      <div className="account-skill-usage">
        <div className="account-skill-usage-heading"><strong>{tr('Skill usage')}</strong><span>{tr('{count} skills', { count: skills.length })}</span></div>
        {skills.length === 0 ? <div className="account-skill-empty">{loading ? tr('Loading') : tr('No skill usage yet.')}</div> : <div className="account-skill-list">
          {skills.map((skill, index) => <div className="account-skill-row" key={skill.id}><span className="account-skill-rank">#{index + 1}</span><strong>{skill.skillName}</strong><span>{skill.countUse.toLocaleString()}</span></div>)}
        </div>}
      </div>
    </>}
  </section>;
}

function buildMonthDays(rows: StatisticRow[], metric: Metric) {
  const now = new Date();
  const year = now.getFullYear();
  const month = now.getMonth();
  const dayCount = new Date(year, month + 1, 0).getDate();
  const values = new Map<number, number>();
  for (const row of rows) {
    const date = new Date(row.createdAt);
    if (Number.isNaN(date.getTime())) continue;
    values.set(date.getUTCDate(), row[metric]);
  }
  return Array.from({ length: dayCount }, (_, index) => {
    const day = index + 1;
    const date = new Date(year, month, day);
    return {
      key: `${year}-${month + 1}-${day}`,
      value: values.get(day) ?? 0,
      offset: index === 0 ? date.getDay() : 0,
      label: new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(date),
    };
  });
}

function heatLevel(value: number, max: number) {
  if (value <= 0 || max <= 0) return 0;
  return Math.min(4, Math.max(1, Math.ceil((value / max) * 4)));
}
