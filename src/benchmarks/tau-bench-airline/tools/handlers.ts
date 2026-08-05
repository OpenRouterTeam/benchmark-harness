import { Either } from "../../../internal/either";
import { isRecord } from "../../../internal/guards";
import type {
  AirlineData,
  PaymentEntry,
  Reservation,
  ReservationFlight,
} from "../types";

type ToolHandler = (
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
) => string;

function str(kwargs: Readonly<Record<string, unknown>>, key: string): string {
  return String(kwargs[key] ?? "");
}

function num(kwargs: Readonly<Record<string, unknown>>, key: string): number {
  return Number(kwargs[key] ?? 0);
}

function arr(
  kwargs: Readonly<Record<string, unknown>>,
  key: string
): readonly Record<string, unknown>[] {
  const val = kwargs[key];
  if (!Array.isArray(val)) {
    return [];
  }
  return val.map((item: unknown) => (isRecord(item) ? item : {}));
}

function toPaymentEntries(
  raw: readonly Record<string, unknown>[]
): PaymentEntry[] {
  return raw.map((p) => ({
    payment_id: String(p["payment_id"] ?? ""),
    amount: Number(p["amount"] ?? 0),
  }));
}

function toReservationFlight(f: Record<string, unknown>): ReservationFlight {
  return {
    flight_number: String(f["flight_number"] ?? ""),
    date: String(f["date"] ?? ""),
    price: Number(f["price"] ?? 0),
    origin: String(f["origin"] ?? ""),
    destination: String(f["destination"] ?? ""),
  };
}

function getUserDetails(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const userId = str(kwargs, "user_id");
  const user = data.users[userId];
  if (!user) {
    return "Error: user not found";
  }
  return JSON.stringify(user);
}

function getReservationDetails(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const reservationId = str(kwargs, "reservation_id");
  const reservation = data.reservations[reservationId];
  if (!reservation) {
    return "Error: user not found";
  }
  return JSON.stringify(reservation);
}

function searchDirectFlight(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const origin = str(kwargs, "origin");
  const destination = str(kwargs, "destination");
  const date = str(kwargs, "date");
  const results: Record<string, unknown>[] = [];
  for (const flight of Object.values(data.flights)) {
    if (flight.origin !== origin || flight.destination !== destination) {
      continue;
    }
    const dateInfo = flight.dates[date];
    if (!dateInfo || dateInfo.status !== "available") {
      continue;
    }
    const { dates: _, ...rest } = flight;
    results.push({ ...rest, ...dateInfo });
  }
  return JSON.stringify(results);
}

function searchOnestopFlight(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const origin = str(kwargs, "origin");
  const destination = str(kwargs, "destination");
  const date = str(kwargs, "date");
  const results: unknown[] = [];
  for (const flight1 of Object.values(data.flights)) {
    if (flight1.origin !== origin) {
      continue;
    }
    for (const flight2 of Object.values(data.flights)) {
      if (
        flight2.destination !== destination ||
        flight1.destination !== flight2.origin
      ) {
        continue;
      }
      if (
        flight1.scheduled_arrival_time_est >
        flight2.scheduled_departure_time_est
      ) {
        continue;
      }
      const date2 = flight1.scheduled_arrival_time_est.includes("+1")
        ? `2024-05-${String(Number(date.slice(-2)) + 1).padStart(2, "0")}`
        : date;
      const dateInfo1 = flight1.dates[date];
      const dateInfo2 = flight2.dates[date2];
      if (!dateInfo1 || dateInfo1.status !== "available") {
        continue;
      }
      if (!dateInfo2 || dateInfo2.status !== "available") {
        continue;
      }
      const { dates: _1, ...rest1 } = flight1;
      const { dates: _2, ...rest2 } = flight2;
      results.push([
        { ...rest1, ...dateInfo1, date },
        { ...rest2, ...dateInfo2, date: date2 },
      ]);
    }
  }
  return JSON.stringify(results);
}

function bookReservation(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const userId = str(kwargs, "user_id");
  const user = data.users[userId];
  if (!user) {
    return "Error: user not found";
  }
  let reservationId = "HATHAT";
  if (data.reservations[reservationId]) {
    reservationId = "HATHAU";
    if (data.reservations[reservationId]) {
      reservationId = "HATHAV";
    }
  }
  const cabin = str(kwargs, "cabin");
  const passengers = arr(kwargs, "passengers");
  const flights = arr(kwargs, "flights").map((f) => ({ ...f }));
  const paymentMethods = toPaymentEntries(arr(kwargs, "payment_methods"));
  const nonfreeBaggages = num(kwargs, "nonfree_baggages");
  const insurance = str(kwargs, "insurance");
  let totalPrice = 0;
  for (const flight of flights) {
    const flightNumber = String(flight["flight_number"] ?? "");
    const flightData = data.flights[flightNumber];
    if (!flightData) {
      return `Error: flight ${flightNumber} not found`;
    }
    const flightDate = String(flight["date"] ?? "");
    const dateData = flightData.dates[flightDate];
    if (!dateData) {
      return `Error: flight ${flightNumber} not found on date ${flightDate}`;
    }
    if (dateData.status !== "available") {
      return `Error: flight ${flightNumber} not available on date ${flightDate}`;
    }
    if ((dateData.available_seats[cabin] ?? 0) < passengers.length) {
      return `Error: not enough seats on flight ${flightNumber}`;
    }
    flight["price"] = dateData.prices[cabin];
    flight["origin"] = flightData.origin;
    flight["destination"] = flightData.destination;
    totalPrice += (dateData.prices[cabin] ?? 0) * passengers.length;
  }
  if (insurance === "yes") {
    totalPrice += 30 * passengers.length;
  }
  totalPrice += 50 * nonfreeBaggages;
  for (const pm of paymentMethods) {
    const paymentId = pm.payment_id;
    const amount = pm.amount;
    const method = user.payment_methods[paymentId];
    if (!method) {
      return `Error: payment method ${paymentId} not found`;
    }
    if (
      (method.source === "gift_card" || method.source === "certificate") &&
      method.amount < amount
    ) {
      return `Error: not enough balance in payment method ${paymentId}`;
    }
  }
  const totalPaid = paymentMethods.reduce((sum, pm) => sum + pm.amount, 0);
  if (totalPaid !== totalPrice) {
    return `Error: payment amount does not add up, total price is ${totalPrice}, but paid ${totalPaid}`;
  }
  for (const pm of paymentMethods) {
    const method = user.payment_methods[pm.payment_id];
    if (!method) {
      continue;
    }
    if (method.source === "gift_card") {
      method.amount -= pm.amount;
    } else if (method.source === "certificate") {
      delete user.payment_methods[pm.payment_id];
    }
  }
  const reservation: Reservation = {
    reservation_id: reservationId,
    user_id: userId,
    origin: str(kwargs, "origin"),
    destination: str(kwargs, "destination"),
    flight_type: str(kwargs, "flight_type"),
    cabin,
    flights: flights.map(toReservationFlight),
    passengers: [...passengers],
    payment_history: [...paymentMethods],
    created_at: "2024-05-15T15:00:00",
    total_baggages: num(kwargs, "total_baggages"),
    nonfree_baggages: nonfreeBaggages,
    insurance,
  };
  data.reservations[reservationId] = reservation;
  user.reservations.push(reservationId);
  return JSON.stringify(reservation);
}

function cancelReservation(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const reservationId = str(kwargs, "reservation_id");
  const reservation = data.reservations[reservationId];
  if (!reservation) {
    return "Error: reservation not found";
  }
  const refunds: PaymentEntry[] = reservation.payment_history.map((p) => ({
    payment_id: p.payment_id,
    amount: -p.amount,
  }));
  reservation.payment_history.push(...refunds);
  reservation.status = "cancelled";
  return JSON.stringify(reservation);
}

function updateReservationFlights(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const reservationId = str(kwargs, "reservation_id");
  const reservation = data.reservations[reservationId];
  if (!reservation) {
    return "Error: reservation not found";
  }
  const cabin = str(kwargs, "cabin");
  const paymentId = str(kwargs, "payment_id");
  const rawFlights = arr(kwargs, "flights").map((f) => ({ ...f }));
  let totalPrice = 0;
  for (const flight of rawFlights) {
    const existing = reservation.flights.find(
      (f) =>
        f.flight_number === flight["flight_number"] &&
        f.date === flight["date"] &&
        cabin === reservation.cabin
    );
    if (existing) {
      totalPrice += existing.price * reservation.passengers.length;
      flight["price"] = existing.price;
      flight["origin"] = existing.origin;
      flight["destination"] = existing.destination;
      continue;
    }
    const flightNumber = String(flight["flight_number"] ?? "");
    const flightData = data.flights[flightNumber];
    if (!flightData) {
      return `Error: flight ${flightNumber} not found`;
    }
    const flightDate = String(flight["date"] ?? "");
    const dateData = flightData.dates[flightDate];
    if (!dateData) {
      return `Error: flight ${flightNumber} not found on date ${flightDate}`;
    }
    if (dateData.status !== "available") {
      return `Error: flight ${flightNumber} not available on date ${flightDate}`;
    }
    if (
      (dateData.available_seats[cabin] ?? 0) < reservation.passengers.length
    ) {
      return `Error: not enough seats on flight ${flightNumber}`;
    }
    flight["price"] = dateData.prices[cabin];
    flight["origin"] = flightData.origin;
    flight["destination"] = flightData.destination;
    totalPrice += (dateData.prices[cabin] ?? 0) * reservation.passengers.length;
  }
  totalPrice -=
    reservation.flights.reduce((sum, f) => sum + f.price, 0) *
    reservation.passengers.length;
  const user = data.users[reservation.user_id];
  if (!user || !(paymentId in user.payment_methods)) {
    return "Error: payment method not found";
  }
  const paymentMethod = user.payment_methods[paymentId]!;
  if (paymentMethod.source === "certificate") {
    return "Error: certificate cannot be used to update reservation";
  }
  if (
    paymentMethod.source === "gift_card" &&
    paymentMethod.amount < totalPrice
  ) {
    return "Error: gift card balance is not enough";
  }
  if (paymentMethod.source === "gift_card") {
    paymentMethod.amount -= totalPrice;
  }
  reservation.flights = rawFlights.map(toReservationFlight);
  if (totalPrice !== 0) {
    reservation.payment_history.push({
      payment_id: paymentId,
      amount: totalPrice,
    });
  }
  return JSON.stringify(reservation);
}

function updateReservationBaggages(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const reservationId = str(kwargs, "reservation_id");
  const reservation = data.reservations[reservationId];
  if (!reservation) {
    return "Error: reservation not found";
  }
  const nonfreeBaggages = num(kwargs, "nonfree_baggages");
  const paymentId = str(kwargs, "payment_id");
  const totalPrice =
    50 * Math.max(0, nonfreeBaggages - reservation.nonfree_baggages);
  const user = data.users[reservation.user_id];
  if (!user || !(paymentId in user.payment_methods)) {
    return "Error: payment method not found";
  }
  const paymentMethod = user.payment_methods[paymentId]!;
  if (paymentMethod.source === "certificate") {
    return "Error: certificate cannot be used to update reservation";
  }
  if (
    paymentMethod.source === "gift_card" &&
    paymentMethod.amount < totalPrice
  ) {
    return "Error: gift card balance is not enough";
  }
  reservation.total_baggages = num(kwargs, "total_baggages");
  reservation.nonfree_baggages = nonfreeBaggages;
  if (paymentMethod.source === "gift_card") {
    paymentMethod.amount -= totalPrice;
  }
  if (totalPrice !== 0) {
    reservation.payment_history.push({
      payment_id: paymentId,
      amount: totalPrice,
    });
  }
  return JSON.stringify(reservation);
}

function updateReservationPassengers(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const reservationId = str(kwargs, "reservation_id");
  const reservation = data.reservations[reservationId];
  if (!reservation) {
    return "Error: reservation not found";
  }
  const passengers = arr(kwargs, "passengers");
  if (passengers.length !== reservation.passengers.length) {
    return "Error: number of passengers does not match";
  }
  reservation.passengers = [...passengers];
  return JSON.stringify(reservation);
}

function listAllAirports(
  _data: AirlineData,
  _kwargs: Readonly<Record<string, unknown>>
): string {
  const airports = [
    "SFO",
    "JFK",
    "LAX",
    "ORD",
    "DFW",
    "DEN",
    "SEA",
    "ATL",
    "MIA",
    "BOS",
    "PHX",
    "IAH",
    "LAS",
    "MCO",
    "EWR",
    "CLT",
    "MSP",
    "DTW",
    "PHL",
    "LGA",
  ];
  const cities = [
    "San Francisco",
    "New York",
    "Los Angeles",
    "Chicago",
    "Dallas",
    "Denver",
    "Seattle",
    "Atlanta",
    "Miami",
    "Boston",
    "Phoenix",
    "Houston",
    "Las Vegas",
    "Orlando",
    "Newark",
    "Charlotte",
    "Minneapolis",
    "Detroit",
    "Philadelphia",
    "LaGuardia",
  ];
  const result: Record<string, string> = {};
  airports.forEach((a, i) => {
    result[a] = cities[i]!;
  });
  return JSON.stringify(result);
}

function sendCertificate(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const userId = str(kwargs, "user_id");
  const user = data.users[userId];
  if (!user) {
    return "Error: user not found";
  }
  const amount = num(kwargs, "amount");
  for (const id of [3221322, 3221323, 3221324]) {
    const paymentId = `certificate_${id}`;
    if (!(paymentId in user.payment_methods)) {
      user.payment_methods[paymentId] = {
        source: "certificate",
        amount,
        id: paymentId,
      };
      return `Certificate ${paymentId} added to user ${userId} with amount ${amount}.`;
    }
  }
  return "Error: cannot add more certificates";
}

function calculate(
  _data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const expression = str(kwargs, "expression");
  if (!/^[0-9+\-*/(). ]+$/.test(expression)) {
    return "Error: invalid characters in expression";
  }
  const evalResult = Either.try(() => new Function(`return (${expression})`)());
  if (Either.isLeft(evalResult)) {
    return `Error: ${String(evalResult.left)}`;
  }
  return String(Math.round(Number(evalResult.right) * 100) / 100);
}

function getFlightStatus(
  data: AirlineData,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const flightNumber = str(kwargs, "flight_number");
  const date = str(kwargs, "date");
  const flight = data.flights[flightNumber];
  if (!flight) {
    return `Error: flight ${flightNumber} not found`;
  }
  const dateInfo = flight.dates[date];
  if (!dateInfo) {
    return `Error: flight ${flightNumber} not found on date ${date}`;
  }
  return dateInfo.status;
}

function transferToHumanAgents(
  _data: AirlineData,
  _kwargs: Readonly<Record<string, unknown>>
): string {
  return "Transfer successful";
}

const TOOL_HANDLERS: Readonly<Record<string, ToolHandler>> = {
  get_user_details: getUserDetails,
  get_reservation_details: getReservationDetails,
  search_direct_flight: searchDirectFlight,
  search_onestop_flight: searchOnestopFlight,
  get_flight_status: getFlightStatus,
  book_reservation: bookReservation,
  cancel_reservation: cancelReservation,
  update_reservation_flights: updateReservationFlights,
  update_reservation_baggages: updateReservationBaggages,
  update_reservation_passengers: updateReservationPassengers,
  list_all_airports: listAllAirports,
  send_certificate: sendCertificate,
  calculate,
  transfer_to_human_agents: transferToHumanAgents,
};

export function invokeTool(
  data: AirlineData,
  name: string,
  kwargs: Readonly<Record<string, unknown>>
): string {
  const handler = TOOL_HANDLERS[name];
  if (!handler) {
    return `Unknown action ${name}`;
  }
  const result = Either.try(() => handler(data, kwargs));
  return Either.isLeft(result) ? `Error: ${String(result.left)}` : result.right;
}
