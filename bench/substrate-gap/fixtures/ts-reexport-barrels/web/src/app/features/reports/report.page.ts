import { Component, OnInit } from '@angular/core';
import { Journal } from 'journal-lib';

@Component({ selector: 'app-report', template: '' })
export class ReportPage implements OnInit {
  constructor(private journal: Journal) {}
  ngOnInit(): void { this.journal.record('open'); }
}
