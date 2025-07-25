import { get } from "wasi:config/store@0.2.0-draft";

export async function getWeather(city) {
  const apiKey = await get("OPENWEATHER_API_KEY");
  if (apiKey === undefined) {
    throw "Error: OPENWEATHER_API_KEY is not set";
  }

  try {
    const geoResponse = await fetch(
      `https://api.openweathermap.org/geo/1.0/direct?q=${city}&limit=1&appid=${apiKey}`
    );
    if (!geoResponse.ok) {
      throw "Error: Failed to fetch geo data";
    }
    const geoData = await geoResponse.json();
    const lat = geoData[0].lat;
    const lon = geoData[0].lon;
    const response = await fetch(
      `https://api.openweathermap.org/data/2.5/weather?lat=${lat}&lon=${lon}&appid=${apiKey}&units=metric`
    );
    if (!response.ok) {
      throw "Error: Failed to fetch weather data";
    }
    const data = await response.json();
    const weather = data.main.temp.toString();
    return weather;  
  } catch (error) {
    throw error.message || "Error fetching weather data";
  }
  
}
