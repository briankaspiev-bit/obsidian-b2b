import { BoothCheckScreen } from '../booth/BoothCheckScreen';
import { HomeScreen } from '../home/HomeScreen';
import { LiveSessionScreen } from '../live-session/LiveSessionScreen';
import { useRoomState } from '../session/useSession';

/** Home → Booth Check → Live Session, driven by the engine's room phase. */
export function App() {
  const { phase } = useRoomState();
  if (phase === 'home') return <HomeScreen />;
  if (phase === 'booth') return <BoothCheckScreen />;
  return <LiveSessionScreen />;
}
