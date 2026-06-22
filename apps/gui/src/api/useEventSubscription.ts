import { useContext } from "react";
import {
  EventSubscriptionContext,
  type EventSubscriptionContextValue,
} from "./EventSubscriptionProvider";

export function useEventSubscription(): EventSubscriptionContextValue {
  return useContext(EventSubscriptionContext);
}
