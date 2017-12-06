/**
 * Blink
 *
 * Turns on an LED on for one second,
 * then off for one second, repeatedly.
 */
#include "Arduino.h"

#ifndef LED_BUILTIN
#define LED_BUILTIN 12
#endif

#define RELAIS_PIN 11

int analogPinToPullupPin(int analogPin) {
  switch(analogPin) {
    case 2: return 7;
    case 3: return 6;
    case 4: return 5;
    case 5: return 4;
    case 6: return 3;
    case 7: return 2;
    default: return 2;
  }

}

void enablePullup(int analogPin) {
  digitalWrite(analogPinToPullupPin(analogPin), HIGH);
}

void disablePullup(int analogPin) {
  digitalWrite(analogPinToPullupPin(analogPin), LOW);
}


void setup()
{

  pinMode(2, OUTPUT); // -> Analog7  oben
  pinMode(3, OUTPUT); // -> Analog6
  pinMode(4, OUTPUT); // -> Analog5
  pinMode(5, OUTPUT); // -> Analog4
  pinMode(6, OUTPUT); // -> Analog3 // unten
  pinMode(7, OUTPUT); // -> Analog2 // außen
  digitalWrite(2, HIGH);
  digitalWrite(3, HIGH);
  digitalWrite(4, HIGH);
  digitalWrite(5, HIGH);
  digitalWrite(6, HIGH);
  digitalWrite(7, HIGH);

  // initialize LED digital pin as an output.
  pinMode(LED_BUILTIN, OUTPUT);
  pinMode(RELAIS_PIN, OUTPUT);
  Serial.begin(9600);
  //Serial.begin(115200);

  Serial.println("Startup!");


}

int temperatures[6]; // 0: Out, 1: LOW, 5: HIGH

void clearTemp() {
  for (int i = 0; i<6; i++) {
    temperatures[i] = 0;
  }
}

void readAddTemperatures() {
  for (int i = 0; i<6; i++) {
    delay(4);
    enablePullup(i+2);
    temperatures[i] += analogRead(i+2);
    disablePullup(i+2);
  }
}

void readTemperatures() {
  clearTemp();
  int avg_over = 20;
  for (int i = 0; i<avg_over; i++) {
    readAddTemperatures();
  }
  for (int i = 0; i<6; i++) {
    temperatures[i] = temperatures[i] / avg_over;
  }

}

void printTemperatures() {
  Serial.print("HIGH:");
  Serial.print(temperatures[5]);
  Serial.print(", HM:");
  Serial.print(temperatures[4]);
  Serial.print(", MID:");
  Serial.print(temperatures[3]);
  Serial.print(", ML:");
  Serial.print(temperatures[2]);
  Serial.print(", LOW:");
  Serial.print(temperatures[1]);
  Serial.print(", OUT:");
  Serial.print(temperatures[0]);
  Serial.println(";");
}

void loop()
{

  readTemperatures();

  // turn the LED on (HIGH is the voltage level)
  digitalWrite(LED_BUILTIN, HIGH);
  // digitalWrite(RELAIS_PIN, HIGH);

  // wait for a second
  delay(20);

  printTemperatures();

  // turn the LED off by making the voltage LOW
  digitalWrite(LED_BUILTIN, LOW);
  // digitalWrite(RELAIS_PIN, LOW);

   // wait for a second
  delay(20);
}
