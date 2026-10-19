import { HealthStatus } from "@/components/HealthStatus";

export default function Home() {
  return (
    <main>
      <h1>mex-scrape</h1>
      <p>
        Compara precios de tiendas mexicanas con Amazon.com.mx, calcula tu ROI
        y consulta el historial de precios.
      </p>
      <p>Próximamente.</p>
      <HealthStatus />
    </main>
  );
}
